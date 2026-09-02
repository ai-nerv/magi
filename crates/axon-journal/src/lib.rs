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

use axon_proto::{Cursor, Entry, SessionId};
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

    /// Replace the last entry in memory, without writing it down yet.
    ///
    /// For a message that is still arriving. [`Journal::amend`] appends a whole record and
    /// flushes, which is right once and ruinous per token: a thousand-token answer would write
    /// the message a thousand times, each copy longer than the last. This keeps the transcript
    /// current — so a UI attaching mid-answer sees what has arrived — and leaves the writing to
    /// the `amend` that ends the message.
    ///
    /// What it costs is the tail of an answer on a crash, which is what the tail of an answer
    /// cost before anything was written down at all.
    pub fn revise(&mut self, entry: Entry) {
        if let Some(last) = self.entries.last_mut() {
            *last = entry;
        }
    }

    /// Where the entry a cursor names sits, if it names one.
    ///
    /// Cursors count from one, so the first entry is at cursor 1. `None` for the zero cursor,
    /// which names the state before anything was written.
    fn at(cursor: Cursor) -> Option<usize> {
        usize::try_from(cursor.0).ok()?.checked_sub(1)
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
        self.amend_at(cursor, entry)?;
        Ok(cursor)
    }

    /// Replace the entry at `cursor`, wherever it is.
    ///
    /// [`Journal::amend`] replaces the *last* entry, which is right for a message that is still
    /// streaming and wrong for anything else. A round of three tool calls commits three entries
    /// and then answers them one at a time: with only the last-entry form, the first two results
    /// landed on the third entry and were then overwritten by it, so two calls kept `result:
    /// null` for the rest of the session. What the model saw was two calls it had made and
    /// never got an answer to.
    ///
    /// The record on disk already carried the cursor it belonged to; nothing read it back.
    ///
    /// # Errors
    /// When the write fails. A cursor naming no entry is ignored rather than refused: it can
    /// only come from a caller holding a cursor from another session, and there is nothing to
    /// amend.
    pub fn amend_at(&mut self, cursor: Cursor, entry: Entry) -> Result<(), JournalError> {
        let Some(at) = Self::at(cursor) else {
            return Ok(());
        };
        let Some(slot) = self.entries.get_mut(at) else {
            return Ok(());
        };
        *slot = entry.clone();
        self.write(&Record::Entry { cursor, entry })?;
        self.writer.flush()?;
        Ok(())
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
    use axon_proto::{MessageId, StopReason};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axon-journal-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("session.jsonl")
    }

    fn user(text: &str) -> Entry {
        Entry::User {
            id: MessageId::new(text),
            text: text.to_owned(),
            aside: String::new(),
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
                signatures: axon_proto::Signatures::default(),
                usage: axon_proto::Usage::default(),
            })
            .expect("append");
        journal
            .amend(Entry::Assistant {
                id: MessageId::new("a1"),
                text: "partial then whole".into(),
                thinking: String::new(),
                stop_reason: Some(StopReason::EndTurn),
                error: None,
                signatures: axon_proto::Signatures::default(),
                usage: axon_proto::Usage::default(),
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

#[cfg(test)]
mod amend_at_tests {
    use super::*;
    use axon_proto::{MessageId, ToolCallId, ToolResult};

    fn journal(name: &str) -> (Journal, PathBuf) {
        let dir = std::env::temp_dir().join(format!("axon-amendat-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("s.jsonl");
        let journal = Journal::open(&path, SessionId::new("s"), "/tmp", 0).expect("open");
        (journal, path)
    }

    fn call(at: usize, answered: bool) -> Entry {
        Entry::Tool {
            id: ToolCallId::new(format!("c{at}")),
            name: "read".into(),
            args: "{}".into(),
            result: answered.then(|| ToolResult {
                output: format!("answer {at}"),
                is_error: false,
            }),
            thought_signature: None,
        }
    }

    /// Which entries have an answer, by their call id.
    fn answered(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool { id, result, .. } => result.as_ref().map(|_| id.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_round_of_calls_each_get_their_own_answer() {
        // The bug this exists for: a round commits every call before running any, so by the time
        // the first result arrives there are two more entries after it. Amending "the last
        // entry" put every result but the last on the wrong one, and the model was left with
        // calls it had made and never got an answer to.
        let (mut journal, _) = journal("round");
        let at: Vec<Cursor> = (0..3)
            .map(|n| journal.append(call(n, false)).expect("append"))
            .collect();

        for (n, cursor) in at.iter().enumerate() {
            journal.amend_at(*cursor, call(n, true)).expect("amend");
        }

        assert_eq!(answered(journal.entries()), vec!["c0", "c1", "c2"]);
    }

    #[test]
    fn an_amendment_to_an_earlier_entry_survives_a_reload() {
        // The record already carried the cursor it belonged to; nothing read it back, so a
        // reload turned each amendment into a fourth, fifth and sixth entry.
        let (mut journal, path) = journal("reload");
        let at: Vec<Cursor> = (0..3)
            .map(|n| journal.append(call(n, false)).expect("append"))
            .collect();
        for (n, cursor) in at.iter().enumerate() {
            journal.amend_at(*cursor, call(n, true)).expect("amend");
        }
        drop(journal);

        let reopened = Journal::open(&path, SessionId::new("s"), "/tmp", 0).expect("reopen");
        assert_eq!(reopened.entries().len(), 3, "three entries, not six");
        assert_eq!(answered(reopened.entries()), vec!["c0", "c1", "c2"]);
    }

    #[test]
    fn amending_the_last_entry_still_works_the_way_it_did() {
        // What a streaming message uses, and the path everything else still takes.
        let (mut journal, _) = journal("last");
        journal
            .append(Entry::User {
                id: MessageId::new("u1"),
                text: "hello".into(),
                aside: String::new(),
            })
            .expect("append");
        journal.append(call(0, false)).expect("append");
        journal.amend(call(0, true)).expect("amend");

        assert_eq!(journal.entries().len(), 2);
        assert_eq!(answered(journal.entries()), vec!["c0"]);
    }

    #[test]
    fn a_cursor_naming_no_entry_is_ignored_rather_than_appended() {
        // It can only come from a caller holding a cursor from another session, and there is
        // nothing to amend.
        let (mut journal, _) = journal("stray");
        journal.append(call(0, false)).expect("append");
        journal.amend_at(Cursor(99), call(9, true)).expect("amend");
        assert_eq!(journal.entries().len(), 1);
        assert!(answered(journal.entries()).is_empty());
    }
}
