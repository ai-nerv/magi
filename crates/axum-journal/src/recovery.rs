//! Reading a journal that a crash may have interrupted.

use crate::record::Record;
use axum_proto::{Cursor, Entry, SessionId};

/// What a journal held, and how much of it was intact.
#[derive(Debug, Default)]
pub struct Recovered {
    /// The header, if the file had one.
    pub meta: Option<Meta>,
    /// The transcript, folded in order.
    pub entries: Vec<Entry>,
    /// Position of the last entry read.
    pub cursor: Cursor,
    /// Bytes covered by complete, valid records.
    pub valid_bytes: usize,
    /// Bytes the file holds. Larger than `valid_bytes` means a torn tail.
    pub total_bytes: usize,
}

/// A journal's header line.
#[derive(Debug, Clone)]
pub struct Meta {
    /// Format version stamped when the journal was created.
    pub version: u16,
    /// The session the file holds.
    pub session: SessionId,
    /// Working directory the session was started in.
    pub cwd: String,
    /// Unix seconds at creation.
    pub started: u64,
}

/// Fold a journal's text into the transcript it describes.
///
/// `Err(line)` names the 1-based line of a record that was complete — it ended in a newline —
/// and did not parse. That is corruption rather than a torn tail, and the caller is expected
/// to refuse the file rather than repair it.
pub fn parse(source: &str) -> Result<Recovered, usize> {
    let mut out = Recovered {
        total_bytes: source.len(),
        ..Recovered::default()
    };

    for (index, line) in source.split_inclusive('\n').enumerate() {
        // A final line with no terminator is a torn tail: the write landed, the newline did
        // not. Everything before it stands, and the caller truncates to `valid_bytes`.
        if !line.ends_with('\n') {
            break;
        }

        let text = line.trim_end();
        if text.is_empty() {
            out.valid_bytes += line.len();
            continue;
        }

        let record: Record = serde_json::from_str(text).map_err(|_| index + 1)?;
        match record {
            Record::Meta {
                version,
                session,
                cwd,
                started,
            } => {
                out.meta = Some(Meta {
                    version,
                    session,
                    cwd,
                    started,
                });
            }
            Record::Entry { cursor, entry } => {
                // A record naming an entry already read is an amendment, and the last one wins.
                // It used to have to name the *last* entry, which held while only a streaming
                // message was ever amended; a round of tool calls answers several entries that
                // are no longer the last, and those amendments were read back as new entries.
                let at = usize::try_from(cursor.0).unwrap_or(0).saturating_sub(1);
                if let Some(slot) = out.entries.get_mut(at) {
                    *slot = entry;
                } else {
                    out.entries.push(entry);
                    out.cursor = cursor;
                }
            }
        }
        out.valid_bytes += line.len();
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str =
        "{\"record\":\"meta\",\"version\":0,\"session\":\"s1\",\"cwd\":\"/tmp\",\"started\":0}\n";

    fn entry_line(cursor: u64, text: &str) -> String {
        format!(
            "{{\"record\":\"entry\",\"cursor\":{cursor},\"entry\":{{\"type\":\"user\",\"id\":\"m{cursor}\",\"text\":\"{text}\"}}}}\n"
        )
    }

    #[test]
    fn an_empty_journal_folds_to_nothing() {
        let out = parse("").expect("parse");
        assert!(out.entries.is_empty());
        assert!(out.meta.is_none());
    }

    #[test]
    fn entries_fold_in_order() {
        let source = format!("{META}{}{}", entry_line(1, "one"), entry_line(2, "two"));
        let out = parse(&source).expect("parse");
        assert_eq!(out.entries.len(), 2);
        assert_eq!(out.cursor, Cursor(2));
        assert_eq!(out.valid_bytes, out.total_bytes);
    }

    #[test]
    fn a_torn_tail_stops_the_fold_and_is_reported_as_uncovered_bytes() {
        let source = format!("{META}{}{{\"record\":\"entry\"", entry_line(1, "one"));
        let out = parse(&source).expect("parse");
        assert_eq!(out.entries.len(), 1);
        assert!(
            out.valid_bytes < out.total_bytes,
            "the fragment is uncovered"
        );
    }

    #[test]
    fn a_complete_but_invalid_record_names_its_line() {
        let source = format!("{META}{}{{\"record\":\"nope\"}}\n", entry_line(1, "one"));
        assert_eq!(parse(&source).expect_err("must fail"), 3);
    }

    #[test]
    fn a_repeated_cursor_amends_rather_than_appends() {
        let source = format!("{META}{}{}", entry_line(1, "draft"), entry_line(1, "final"));
        let out = parse(&source).expect("parse");
        assert_eq!(out.entries.len(), 1, "an amendment is not a second entry");
        match &out.entries[0] {
            Entry::User { text, .. } => assert_eq!(text, "final", "the last write wins"),
            other => panic!("expected a user entry, got {other:?}"),
        }
    }

    #[test]
    fn blank_lines_are_tolerated() {
        let source = format!("{META}\n{}", entry_line(1, "one"));
        let out = parse(&source).expect("parse");
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.valid_bytes, out.total_bytes);
    }

    #[test]
    fn the_meta_line_is_recovered() {
        let out = parse(META).expect("parse");
        let meta = out.meta.expect("meta");
        assert_eq!(meta.version, 0);
        assert_eq!(meta.session.as_str(), "s1");
        assert_eq!(meta.cwd, "/tmp");
    }
}
