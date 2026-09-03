//! Handing the transcript to balthasar.
//!
//! Beside [`crate::session::Session`] rather than inside it. A session is held behind a mutex
//! and a commit is a short lock; putting a socket round trip in there would hold that lock
//! across balthasar's `fsync` and freeze every UI read for the length of it. So the session
//! commits to memory, and the turn driver hands the settled entry over next, outside the lock.
//!
//! What travels is `raw` — the serialised [`Record`], byte for byte, which balthasar stores and
//! never parses. The other fields are a projection for balthasar's own searching and quoting;
//! losing one costs a nicer diagnostic rather than a session.

use magi_ipc::family::{Family, Fault};
use magi_journal::{JOURNAL_VERSION, Record};
use magi_proto::{Cursor, Entry, SessionId};

/// A connection to balthasar, bound to one session.
pub struct Scribe {
    family: Family,
    session: String,
}

impl Scribe {
    /// Find a running balthasar and bind to a session.
    pub async fn find(session: &SessionId) -> Result<Self, Fault> {
        Ok(Self {
            family: Family::find(None).await?,
            session: session.as_str().to_owned(),
        })
    }

    /// Bind to a session over an already-open connection.
    #[must_use]
    pub fn over(family: Family, session: &SessionId) -> Self {
        Self {
            family,
            session: session.as_str().to_owned(),
        }
    }

    /// Record a settled entry. Durable when this returns.
    pub async fn observe(&mut self, cursor: Cursor, entry: &Entry) -> Result<(), Fault> {
        self.write("observe", cursor, entry).await
    }

    /// Revise the entry already at this cursor.
    pub async fn amend(&mut self, cursor: Cursor, entry: &Entry) -> Result<(), Fault> {
        self.write("amend", cursor, entry).await
    }

    async fn write(&mut self, verb: &str, cursor: Cursor, entry: &Entry) -> Result<(), Fault> {
        let turn = turn(cursor, entry)?;
        self.family
            .call(
                verb,
                vec![serde_json::Value::String(self.session.clone()), turn],
            )
            .await
            .map(|_| ())
    }

    /// Everything this session said, in cursor order, as it finally stood.
    pub async fn replay(&mut self) -> Result<Vec<(Cursor, Entry)>, Fault> {
        let values = self
            .family
            .call(
                "replay",
                vec![serde_json::Value::String(self.session.clone())],
            )
            .await?;
        values.iter().flat_map(rows).map(rebuild).collect()
    }

    /// The runs this project has had.
    pub async fn sessions(&mut self) -> Result<Vec<serde_json::Value>, Fault> {
        let values = self.family.call("sessions", Vec::new()).await?;
        Ok(values.iter().flat_map(rows).cloned().collect())
    }
}

/// A reply value that is a list of rows, or the single row it is.
fn rows(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    value
        .as_array()
        .map_or_else(|| vec![value], |a| a.iter().collect())
}

/// Rebuild one entry from the `raw` balthasar handed back.
///
/// Read from `raw` and nothing else. The projection is balthasar's to search; the record is
/// magi's, and reconstructing from a projection would quietly lose every field the projection
/// does not carry.
fn rebuild(row: &serde_json::Value) -> Result<(Cursor, Entry), Fault> {
    let raw = row
        .get("raw")
        .ok_or_else(|| Fault::Malformed("a replayed row has no raw".into()))?;

    // Either a JSON object or the string it was serialised to; balthasar accepts both, so both
    // can come back.
    let record: Record = match raw {
        serde_json::Value::String(text) => serde_json::from_str(text),
        other => serde_json::from_value(other.clone()),
    }
    .map_err(|e| Fault::Malformed(format!("raw is not a record: {e}")))?;

    match record {
        Record::Entry { cursor, entry } => Ok((cursor, entry)),
        Record::Meta { version, .. } => Err(Fault::Malformed(format!(
            "a meta record replayed as an entry (version {version}, this build writes {JOURNAL_VERSION})"
        ))),
    }
}

/// The wire shape of one settled entry.
fn turn(cursor: Cursor, entry: &Entry) -> Result<serde_json::Value, Fault> {
    let record = Record::Entry {
        cursor,
        entry: entry.clone(),
    };
    let raw = serde_json::to_value(&record)
        .map_err(|e| Fault::Malformed(format!("a record would not serialise: {e}")))?;

    let mut turn = serde_json::Map::new();
    turn.insert("cursor".into(), serde_json::Value::from(cursor.0));
    turn.insert("role".into(), serde_json::Value::from(role(entry)));
    turn.insert("kind".into(), serde_json::Value::from(kind(entry)));
    turn.insert("text".into(), serde_json::Value::from(text(entry)));
    if let Entry::Tool { name, .. } = entry {
        turn.insert("tool".into(), serde_json::Value::from(name.clone()));
    }
    turn.insert("raw".into(), raw);
    Ok(serde_json::Value::Object(turn))
}

/// Who is speaking.
fn role(entry: &Entry) -> &'static str {
    match entry {
        Entry::User { .. } | Entry::From { .. } => "user",
        Entry::Assistant { .. } | Entry::Notice { .. } => "assistant",
        Entry::Tool { .. } => "tool",
        Entry::Branch { .. } | Entry::Compaction { .. } => "system",
    }
}

/// Which block this is.
///
/// `user`, `from` and `branch` are beyond the five `PLAN_SCROLLBACK.md` named. Without them a
/// message from a sibling session is filed as ordinary prose and quoted back as though the
/// person had typed it.
fn kind(entry: &Entry) -> &'static str {
    match entry {
        Entry::User { .. } => "user",
        Entry::From { .. } => "from",
        Entry::Branch { .. } => "branch",
        Entry::Compaction { .. } => "summary",
        Entry::Tool { result: None, .. } => "tool_call",
        Entry::Tool { .. } => "tool_result",
        Entry::Assistant { text, thinking, .. } if text.is_empty() && !thinking.is_empty() => {
            "thinking"
        }
        Entry::Assistant { .. } | Entry::Notice { .. } => "prose",
    }
}

/// What balthasar quotes and searches. A projection, never the record.
fn text(entry: &Entry) -> String {
    match entry {
        Entry::User { text, .. }
        | Entry::From { text, .. }
        | Entry::Notice { text }
        | Entry::Compaction { summary: text, .. } => text.clone(),
        Entry::Assistant { text, thinking, .. } if text.is_empty() => thinking.clone(),
        Entry::Assistant { text, .. } => text.clone(),
        Entry::Tool {
            name, args, result, ..
        } => match result {
            Some(done) => done.output.clone(),
            None => format!("{name} {args}"),
        },
        Entry::Branch { keeps, .. } => format!("branched, keeping {keeps}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::{MessageId, ToolCallId, ToolResult};

    fn assistant(text: &str, thinking: &str) -> Entry {
        Entry::Assistant {
            id: MessageId::new("a1"),
            text: text.into(),
            thinking: thinking.into(),
            stop_reason: None,
            error: None,
            signatures: Default::default(),
            usage: Default::default(),
        }
    }

    #[test]
    fn every_variant_gets_a_kind_of_its_own_where_it_needs_one() {
        let user = Entry::User {
            id: MessageId::new("u1"),
            text: "hi".into(),
            aside: String::new(),
        };
        let from = Entry::From {
            who: "p/x".into(),
            kin: "sibling".into(),
            sort: "question".into(),
            text: "hi".into(),
        };
        assert_eq!(kind(&user), "user");
        assert_eq!(kind(&from), "from");
        assert_ne!(kind(&user), kind(&from), "a sibling is not the person");
    }

    #[test]
    fn a_tool_changes_kind_when_its_result_lands() {
        let mut call = Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "shell".into(),
            args: "{}".into(),
            result: None,
            thought_signature: None,
        };
        assert_eq!(kind(&call), "tool_call");
        if let Entry::Tool { result, .. } = &mut call {
            *result = Some(ToolResult {
                output: "done".into(),
                is_error: false,
            });
        }
        assert_eq!(kind(&call), "tool_result");
    }

    #[test]
    fn a_message_that_is_only_reasoning_is_thinking_rather_than_prose() {
        assert_eq!(kind(&assistant("", "mulling")), "thinking");
        assert_eq!(kind(&assistant("said", "mulling")), "prose");
    }

    #[test]
    fn the_raw_record_is_what_travels_and_it_round_trips() {
        let entry = assistant("said", "mulling");
        let wire = turn(Cursor(7), &entry).expect("turn");
        assert_eq!(wire["cursor"], serde_json::json!(7));

        let (cursor, back) = rebuild(&wire).expect("rebuild");
        assert_eq!(cursor, Cursor(7));
        assert_eq!(back, entry, "the entry must survive the wire unaltered");
    }

    #[test]
    fn a_row_whose_raw_is_a_string_rebuilds_the_same_as_one_that_is_an_object() {
        let entry = assistant("said", "");
        let wire = turn(Cursor(2), &entry).expect("turn");
        let as_text = serde_json::json!({
            "raw": serde_json::to_string(&wire["raw"]).expect("stringify"),
        });
        assert_eq!(rebuild(&as_text).expect("rebuild"), (Cursor(2), entry));
    }

    #[test]
    fn a_row_with_no_raw_is_malformed_rather_than_an_empty_entry() {
        let row = serde_json::json!({ "cursor": 1, "text": "hi", "kind": "user" });
        assert!(matches!(rebuild(&row), Err(Fault::Malformed(_))));
    }

    #[test]
    fn the_projection_never_stands_in_for_the_record() {
        // A signature is in `raw` and nowhere else; rebuilding from `text` would lose it and
        // the next provider call would be a 400.
        let entry = Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "shell".into(),
            args: "{\"command\":\"ls\"}".into(),
            result: None,
            thought_signature: Some("opaque-signature".into()),
        };
        let wire = turn(Cursor(3), &entry).expect("turn");
        let shown = wire["text"].as_str().expect("text is a string");
        assert!(
            !shown.contains("opaque-signature"),
            "the signature leaked into the projection"
        );
        let (_, back) = rebuild(&wire).expect("rebuild");
        assert_eq!(back, entry);
    }
}
