//! What a journal line holds.

use magi_proto::{Cursor, Entry, SessionId};
use serde::{Deserialize, Serialize};

/// The journal format. Stays `0`: while magi is the only reader, breaking it is free.
pub const JOURNAL_VERSION: u16 = 0;

/// One line of a journal. Entries are stored, not events: a completed assistant message is one
/// line rather than the hundreds of deltas that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "record")]
pub enum Record {
    Meta {
        version: u16,
        session: SessionId,
        cwd: String,
        started: u64,
    },
    Entry {
        cursor: Cursor,
        entry: Entry,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::MessageId;

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
