//! An append-only session journal.
//!
//! One JSONL file per session: a `meta` line, then one line per transcript entry. Greppable
//! on purpose — Tau stores CBOR and then built a second JSONL log to read it, which is the
//! argument settled before it was had.
//!
//! Nothing is ever deleted. Compaction will be an entry that names where the kept history
//! starts, and branching a pointer move; neither rewrites a line that is already on disk.

mod record;
mod recovery;

pub use record::{JOURNAL_VERSION, Record};
pub use recovery::{Recovered, parse};

use axum_proto::{Cursor, Entry, SessionId};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Anything that can go wrong reading or writing a journal.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// The filesystem operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A line was complete — it ended in a newline — and did not parse.
    ///
    /// This is corruption, not a torn tail, and repairing it would silently drop history. The
    /// load fails instead. The offending text is deliberately absent: a line that failed to
    /// parse is the least trustworthy thing in the file and the most likely to reach a log.
    #[error("journal {path} is corrupt at line {line}")]
    Corrupt {
        /// The journal that failed to load.
        path: PathBuf,
        /// 1-based line number of the offending record.
        line: usize,
    },

    /// The journal was written by a build that does not agree with this one.
    #[error("journal {path} is version {found}, this build writes {JOURNAL_VERSION}")]
    Version {
        /// The journal that failed to load.
        path: PathBuf,
        /// The version stamped on its meta line.
        found: u16,
    },

    /// The first line was not a `meta` record.
    #[error("journal {path} has no meta line")]
    NoMeta {
        /// The journal that failed to load.
        path: PathBuf,
    },
}

/// An open session journal.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    writer: BufWriter<File>,
    session: SessionId,
    entries: Vec<Entry>,
    next: Cursor,
}

impl Journal {
    /// Open an existing journal, or create one.
    ///
    /// A torn tail — a final line with no newline, which is what a crash between the write and
    /// the terminator leaves — is truncated. A complete line that does not parse is not.
    pub fn open(
        path: &Path,
        session: SessionId,
        cwd: &str,
        now: u64,
    ) -> Result<Self, JournalError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let recovered = match std::fs::read_to_string(path) {
            Ok(source) => Some(parse(&source).map_err(|line| JournalError::Corrupt {
                path: path.to_owned(),
                line,
            })?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };

        let mut journal = match recovered {
            Some(recovered) => {
                let meta = recovered.meta.ok_or_else(|| JournalError::NoMeta {
                    path: path.to_owned(),
                })?;
                if meta.version != JOURNAL_VERSION {
                    return Err(JournalError::Version {
                        path: path.to_owned(),
                        found: meta.version,
                    });
                }
                // A torn tail leaves bytes past the last good record. Truncating on open means
                // the next append lands on a boundary rather than after a fragment.
                if recovered.valid_bytes < recovered.total_bytes {
                    let file = OpenOptions::new().write(true).open(path)?;
                    file.set_len(recovered.valid_bytes as u64)?;
                }
                Self {
                    path: path.to_owned(),
                    writer: BufWriter::new(OpenOptions::new().append(true).open(path)?),
                    session: meta.session,
                    next: recovered.cursor.next(),
                    entries: recovered.entries,
                }
            }
            None => {
                let mut journal = Self {
                    path: path.to_owned(),
                    writer: BufWriter::new(
                        OpenOptions::new().create(true).append(true).open(path)?,
                    ),
                    session: session.clone(),
                    entries: Vec::new(),
                    next: Cursor::ZERO.next(),
                };
                journal.write(&Record::Meta {
                    version: JOURNAL_VERSION,
                    session,
                    cwd: cwd.to_owned(),
                    started: now,
                })?;
                journal
            }
        };
        journal.writer.flush()?;
        Ok(journal)
    }

    /// The session this journal holds.
    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Where the journal lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The transcript, in order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The position of the last entry written.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor(self.next.0.saturating_sub(1))
    }

    /// Append an entry and return the position it took.
    pub fn append(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        let cursor = self.next;
        self.write(&Record::Entry {
            cursor,
            entry: entry.clone(),
        })?;
        // Flushed per entry: an entry that reached the UI but not the disk is a session that
        // resumes missing something the user watched happen.
        self.writer.flush()?;
        self.entries.push(entry);
        self.next = cursor.next();
        Ok(cursor)
    }

    /// Replace the last entry, for a message that was still streaming when it settled.
    ///
    /// Appends rather than rewriting: the reader keeps the last record for a given cursor, so
    /// history stays append-only and a crash mid-update leaves the previous version intact.
    pub fn amend(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        if self.entries.is_empty() {
            return self.append(entry);
        }
        let cursor = self.cursor();
        if let Some(last) = self.entries.last_mut() {
            *last = entry.clone();
        }
        self.write(&Record::Entry { cursor, entry })?;
        self.writer.flush()?;
        Ok(cursor)
    }

    fn write(&mut self, record: &Record) -> Result<(), JournalError> {
        let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
        self.writer.write_all(line.as_bytes())?;
        // The terminator is what makes a record complete. Written after the body, so a crash
        // between the two leaves a torn tail the reader can recognise and drop.
        self.writer.write_all(b"\n")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_proto::{MessageId, StopReason};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axum-journal-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("session.jsonl")
    }

    fn user(text: &str) -> Entry {
        Entry::User {
            id: MessageId::new(text),
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_new_journal_starts_with_a_meta_line() {
        let path = temp("meta");
        let journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 7).expect("open");
        assert_eq!(journal.session().as_str(), "s1");
        let source = std::fs::read_to_string(&path).expect("read");
        assert!(source.starts_with(r#"{"record":"meta""#), "{source}");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn appended_entries_survive_a_reopen() {
        let path = temp("reopen");
        {
            let mut journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("open");
            journal.append(user("one")).expect("append");
            journal.append(user("two")).expect("append");
        }
        let journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("reopen");
        assert_eq!(journal.entries().len(), 2);
        assert_eq!(journal.cursor(), Cursor(2));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn cursors_continue_where_the_file_left_off() {
        let path = temp("cursors");
        {
            let mut journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("open");
            assert_eq!(journal.append(user("one")).expect("append"), Cursor(1));
        }
        let mut journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("reopen");
        assert_eq!(journal.append(user("two")).expect("append"), Cursor(2));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_torn_tail_is_truncated_and_the_journal_stays_writable() {
        let path = temp("torn");
        {
            let mut journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("open");
            journal.append(user("kept")).expect("append");
        }
        // A crash between writing a record and its terminator.
        let mut source = std::fs::read_to_string(&path).expect("read");
        source.push_str(r#"{"record":"entry","cursor":2,"entry":{"type":"user""#);
        std::fs::write(&path, &source).expect("write");

        let mut journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("reopen");
        assert_eq!(journal.entries().len(), 1, "the fragment is dropped");
        journal
            .append(user("next"))
            .expect("append after truncation");

        let reopened = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("reopen");
        assert_eq!(
            reopened.entries().len(),
            2,
            "the append landed on a boundary"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_complete_but_invalid_record_fails_the_load() {
        let path = temp("corrupt");
        {
            let mut journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("open");
            journal.append(user("kept")).expect("append");
        }
        let mut source = std::fs::read_to_string(&path).expect("read");
        source.push_str("{\"record\":\"nonsense\"}\n");
        std::fs::write(&path, &source).expect("write");

        let error = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect_err("must fail");
        assert!(
            matches!(error, JournalError::Corrupt { line: 3, .. }),
            "{error:?}"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn amending_replaces_the_last_entry_without_rewriting_the_file() {
        let path = temp("amend");
        let mut journal = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("open");
        journal
            .append(Entry::Assistant {
                id: MessageId::new("a1"),
                text: "par".into(),
                thinking: String::new(),
                stop_reason: None,
                error: None,
                signatures: axum_proto::Signatures::default(),
            })
            .expect("append");
        journal
            .amend(Entry::Assistant {
                id: MessageId::new("a1"),
                text: "partial then whole".into(),
                thinking: String::new(),
                stop_reason: Some(StopReason::EndTurn),
                error: None,
                signatures: axum_proto::Signatures::default(),
            })
            .expect("amend");
        drop(journal);

        let reopened = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect("reopen");
        assert_eq!(
            reopened.entries().len(),
            1,
            "an amendment is not a new entry"
        );
        match &reopened.entries()[0] {
            Entry::Assistant {
                text, stop_reason, ..
            } => {
                assert_eq!(text, "partial then whole");
                assert_eq!(*stop_reason, Some(StopReason::EndTurn));
            }
            other => panic!("expected an assistant entry, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_journal_from_a_future_version_is_refused() {
        let path = temp("version");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &path,
            "{\"record\":\"meta\",\"version\":99,\"session\":\"s1\",\"cwd\":\"/\",\"started\":0}\n",
        )
        .expect("write");
        let error = Journal::open(&path, SessionId::new("s1"), "/tmp", 0).expect_err("must fail");
        assert!(
            matches!(error, JournalError::Version { found: 99, .. }),
            "{error:?}"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }
}
