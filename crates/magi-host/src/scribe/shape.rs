//! What one entry looks like on the memory layer's wire, and what comes back from it.

use super::Beside;
use magi_ipc::family::Fault;
use magi_journal::{JOURNAL_VERSION, Record};
use magi_proto::{Cursor, Entry};

/// What a recall answered, and the ledger entry it belongs to. balthasar answers `recall` in two
/// shapes — a bare list with its ledger off, `{ injection, memories }` with it on — and reading
/// only the first is how the automatic path came to carry no injection id at all.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Recalled {
    pub memories: Vec<serde_json::Value>,
    pub injection: Option<String>,
}

impl Recalled {
    /// Read whichever shape came back.
    pub(super) fn of(values: &[serde_json::Value]) -> Self {
        let Some(first) = values.first() else {
            return Self::default();
        };
        if let Some(id) = first.get("injection").and_then(serde_json::Value::as_str) {
            return Self {
                memories: first
                    .get("memories")
                    .map(|m| rows(m).into_iter().cloned().collect())
                    .unwrap_or_default(),
                injection: Some(id.to_owned()),
            };
        }
        Self {
            memories: values.iter().flat_map(rows).cloned().collect(),
            injection: None,
        }
    }
}

/// A reply value that is a list of rows, or the single row it is.
pub(super) fn rows(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    value
        .as_array()
        .map_or_else(|| vec![value], |a| a.iter().collect())
}

/// Rebuild one entry from the `raw` balthasar handed back, and nothing else: reconstructing from
/// the projection would quietly lose every field the projection does not carry. A row marked
/// `truncated` has no `raw` and is rebuilt by [`cut_short`] instead.
pub(super) fn rebuild(row: &serde_json::Value) -> Result<(Cursor, Entry), Fault> {
    if row.get("truncated").and_then(serde_json::Value::as_bool) == Some(true) {
        return cut_short(row);
    }
    let raw = row
        .get("raw")
        .ok_or_else(|| Fault::Malformed("a replayed row has no raw".into()))?;

    // Either a JSON object or the string it was serialised to; balthasar accepts both.
    let record: Record = match raw {
        serde_json::Value::String(text) => serde_json::from_str(text),
        other => serde_json::from_value(other.clone()),
    }
    .map_err(|e| Fault::Malformed(format!("raw is not a record: {e}")))?;

    match record {
        Record::Entry { cursor, entry } => {
            if let Some(advertised) = row.get("cursor")
                && advertised.as_u64() != Some(cursor.0)
            {
                return Err(Fault::Malformed(
                    "replay cursor differs from raw record".into(),
                ));
            }
            Ok((cursor, entry))
        }
        Record::Meta { version, .. } => Err(Fault::Malformed(format!(
            "a meta record replayed as an entry (version {version}, this build writes {JOURNAL_VERSION})"
        ))),
    }
}

/// The marker left on an entry rebuilt from a row the memory layer cut to fit one frame.
pub(crate) const CUT_SHORT: &str =
    "\n[cut short: too large for one frame, so only this much of it was kept]";

/// An entry from a row cut to fit one frame: no record, its text shortened. What remains is kept as
/// what was said by whoever said it, marked with [`CUT_SHORT`]; a kind with no text of its own to
/// carry it becomes a notice.
fn cut_short(row: &serde_json::Value) -> Result<(Cursor, Entry), Fault> {
    use magi_proto::{MessageId, ToolCallId, ToolResult};
    let field = |name: &str| row.get(name).and_then(serde_json::Value::as_str);
    let cursor = row
        .get("cursor")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Fault::Malformed("a cut-short row has no cursor".into()))?;
    let text = format!("{}{CUT_SHORT}", field("text").unwrap_or_default());
    let id = format!("cut-{cursor}");
    let entry = match (field("role").unwrap_or_default(), field("kind")) {
        ("user", _) => Entry::User {
            id: MessageId::new(id),
            text,
            aside: String::new(),
        },
        ("assistant", Some("prose" | "thinking")) => Entry::Assistant {
            id: MessageId::new(id),
            text,
            thinking: String::new(),
            stop_reason: Some(magi_model::StopReason::EndTurn),
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        },
        ("tool", kind) => Entry::Tool {
            id: ToolCallId::new(id),
            name: field("tool").unwrap_or("unknown").to_owned(),
            args: serde_json::json!({ "not_recorded": CUT_SHORT.trim() }).to_string(),
            result: (kind != Some("tool_call")).then(|| ToolResult {
                output: text,
                is_error: row.get("error").and_then(serde_json::Value::as_bool) == Some(true)
                    || row.get("ok").and_then(serde_json::Value::as_bool) == Some(false),
                shown: None,
            }),
            thought_signature: None,
        },
        _ => Entry::Notice { text },
    };
    Ok((Cursor(cursor), entry))
}

/// What travels beside the entry at `cursor`. A tool row's group is the message that called it.
pub(super) fn beside(session: &crate::session::Session, cursor: Cursor, entry: &Entry) -> Beside {
    match entry {
        Entry::Assistant { .. } => Beside {
            group: Some(cursor.0),
            ..Beside::default()
        },
        Entry::Tool { id, .. } => {
            let at = session.position(cursor).unwrap_or(0);
            let entries = session.entries();
            let before = entries.get(..at).unwrap_or(entries);
            Beside {
                group: before
                    .iter()
                    .rposition(|e| matches!(e, Entry::Assistant { .. }))
                    .and_then(|i| session.cursor_at(i).map(|c| c.0)),
                hints: session.hints(id.as_str()),
            }
        }
        _ => Beside::default(),
    }
}

/// The wire shape of one settled entry.
pub(super) fn turn(
    cursor: Cursor,
    entry: &Entry,
    beside: &Beside,
) -> Result<serde_json::Value, Fault> {
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
    turn.insert("tokens".into(), serde_json::Value::from(tokens(entry)));
    if let Some(group) = beside.group {
        turn.insert("group".into(), serde_json::Value::from(group));
    }
    if let Entry::Tool {
        name, args, result, ..
    } = entry
    {
        turn.insert("tool".into(), serde_json::Value::from(name.clone()));
        // What was asked for, beside what came back: a failure means little without it, and it is
        // what a summary carries of a call that failed.
        let asked =
            serde_json::from_str(args).unwrap_or_else(|_| serde_json::Value::from(args.clone()));
        turn.insert("args".into(), asked);
        let failed = result.as_ref().is_some_and(|r| r.is_error);
        turn.insert("error".into(), serde_json::Value::from(failed));
        turn.insert(
            "keep".into(),
            serde_json::Value::from(beside.hints.keep || failed),
        );
        if let Some(brief) = &beside.hints.brief {
            turn.insert("stub".into(), serde_json::Value::from(brief.clone()));
        }
        if let Some(back) = &beside.hints.back {
            turn.insert("handle".into(), serde_json::Value::from(back.clone()));
        }
    }
    turn.insert("raw".into(), raw);
    Ok(serde_json::Value::Object(turn))
}

/// The entry with every text in it cut to `keep` bytes and marked, and nothing opaque left in it:
/// what is recorded of an entry too large for one frame.
pub(super) fn stubbed(entry: &Entry, keep: usize) -> Entry {
    let mut out = entry.clone();
    match &mut out {
        Entry::User { text, aside, .. } => {
            cut(text, keep);
            cut(aside, keep);
        }
        Entry::Assistant {
            text,
            thinking,
            error,
            signatures,
            ..
        } => {
            cut(text, keep);
            cut(thinking, keep);
            if let Some(error) = error {
                cut(error, keep);
            }
            *signatures = magi_proto::Signatures::default();
        }
        Entry::Tool {
            args,
            result,
            thought_signature,
            ..
        } => {
            if args.len() > keep {
                *args = serde_json::json!({ "not_recorded": cut_note(args.len()) }).to_string();
            }
            if let Some(result) = result {
                cut(&mut result.output, keep);
                result.shown = None;
            }
            *thought_signature = None;
        }
        Entry::Notice { text } | Entry::From { text, .. } => cut(text, keep),
        Entry::Compaction { summary, .. } => cut(summary, keep),
        Entry::Masked { shown, .. } => cut(shown, keep),
        Entry::Branch { .. } => {}
    }
    out
}

/// Cut `text` to at most `keep` bytes on a character boundary, saying how much went.
pub(crate) fn cut(text: &mut String, keep: usize) {
    if text.len() <= keep {
        return;
    }
    let mut at = keep;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let gone = text.len() - at;
    text.truncate(at);
    text.push_str(&cut_note(gone));
}

/// The marker left where text was cut.
fn cut_note(gone: usize) -> String {
    format!("\n[{gone} more bytes cut here: past the family's frame cap]")
}

/// What sending this entry costs: an estimate of everything the model is shown of it.
pub(crate) fn tokens(entry: &Entry) -> u64 {
    use magi_model::estimate::tokens as of;
    match entry {
        Entry::User { text, aside, .. } => of(text) + of(aside),
        Entry::Assistant { text, thinking, .. } => of(text) + of(thinking),
        Entry::Tool {
            name, args, result, ..
        } => of(name) + of(args) + result.as_ref().map_or(0, |r| of(&r.output)),
        other => of(&text(other)),
    }
}

fn role(entry: &Entry) -> &'static str {
    match entry {
        Entry::User { .. } | Entry::From { .. } => "user",
        Entry::Assistant { .. } | Entry::Notice { .. } => "assistant",
        Entry::Tool { .. } => "tool",
        Entry::Branch { .. } | Entry::Compaction { .. } | Entry::Masked { .. } => "system",
    }
}

/// Which block this is. `user`, `from` and `branch` are beyond the five `PLAN_SCROLLBACK.md` named;
/// without them a sibling's message is quoted back as though the person had typed it.
pub(super) fn kind(entry: &Entry) -> &'static str {
    match entry {
        Entry::User { .. } => "user",
        Entry::From { .. } => "from",
        Entry::Branch { .. } => "branch",
        Entry::Compaction { .. } => "summary",
        // Never streamed — see `flush`. Named anyway, so the day one is, it is not filed as prose.
        Entry::Masked { .. } => "mask",
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
        | Entry::Compaction { summary: text, .. }
        | Entry::Masked { shown: text, .. } => text.clone(),
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
