//! What a journal line holds.

use axon_proto::{Cursor, Entry, SessionId};
use serde::{Deserialize, Serialize};

/// The journal format. Stays `0`: while axon is the only reader, breaking it is free.
pub const JOURNAL_VERSION: u16 = 0;

/// One line of a journal.
///
/// Entries are stored, not events. A completed assistant message is one line rather than the
/// hundreds of deltas that produced it — Pi does the same, and it is the difference between a
/// session file you can `less` and one you cannot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "record")]
pub enum Record {
    /// The first line of every journal.
    Meta {
        /// Always [`JOURNAL_VERSION`] for journals this build writes.
        version: u16,
        /// The session this file holds.
        session: SessionId,
        /// Working directory the session was started in.
        cwd: String,
        /// Unix seconds at creation.
        started: u64,
    },
    /// A transcript entry, in the order it settled.
    Entry {
        /// Position in the log; also what a UI resumes from.
        cursor: Cursor,
        /// The entry itself.
        entry: Entry,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_proto::MessageId;

    #[test]
    fn a_meta_record_round_trips() {
        let record = Record::Meta {
            version: JOURNAL_VERSION,
            session: SessionId::new("s1"),
            cwd: "/tmp".into(),
            started: 42,
        };
        let line = serde_json::to_string(&record).expect("encode");
        assert_eq!(
            serde_json::from_str::<Record>(&line).expect("decode"),
            record
        );
    }

    #[test]
    fn an_entry_record_round_trips() {
        let record = Record::Entry {
            cursor: Cursor(3),
            entry: Entry::User {
                id: MessageId::new("m1"),
                text: "hi".into(),
                aside: String::new(),
            },
        };
        let line = serde_json::to_string(&record).expect("encode");
        assert_eq!(
            serde_json::from_str::<Record>(&line).expect("decode"),
            record
        );
    }

    #[test]
    fn a_record_is_one_line() {
        let record = Record::Entry {
            cursor: Cursor(1),
            entry: Entry::User {
                id: MessageId::new("m1"),
                text: "two\nlines".into(),
                aside: String::new(),
            },
        };
        let line = serde_json::to_string(&record).expect("encode");
        assert!(
            !line.contains('\n'),
            "a newline in the text must be escaped"
        );
    }
}
