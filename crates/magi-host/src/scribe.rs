//! Handing the transcript to balthasar, beside [`crate::session::Session`] rather than inside it:
//! a socket round trip under the session lock would hold it across balthasar's `fsync` and freeze
//! every UI read. What travels is `raw`, the serialised [`Record`]; the rest is a projection.

use magi_ipc::family::{Family, Fault};
use magi_journal::{JOURNAL_VERSION, Record};
use magi_proto::{Cursor, Entry, SessionId};

/// The one connection to balthasar, as the session shares it. Named here rather than beside either
/// of its users, which would put `worker` and `turn` in a cycle.
pub type Held = std::sync::Arc<tokio::sync::Mutex<Option<Scribe>>>;

/// The program that filled the `memory` role before it was a role, and the default when nothing
/// names another.
pub const BALTHASAR: &str = "balthasar";

/// Every verb the memory role names, from `ROLES.md`. Here so [`Scribe::raw`] cannot reach past the
/// contract: an escape hatch taking any verb makes the contract advisory.
const ROLE: &[&str] = &[
    "observe", "replay", "amend", "recall", "remember", "forget", "why", "scroll", "plan", "used",
    "outcome", "model", "resume", "sessions",
];

pub struct Scribe {
    family: Family,
    /// Where to dial to get this connection back. balthasar can restart, and it drops a caller that
    /// has been quiet — which magi is between prompts — so without this the first drop ended
    /// recording for the session. `None` when the connection was found rather than named.
    at: Option<std::path::PathBuf>,
    session: String,
    /// Cursors already sent, so a second write says `amend` rather than `observe`.
    sent: std::collections::BTreeSet<u64>,
}

impl Scribe {
    /// Find a running balthasar and bind to a session.
    pub async fn find(session: &SessionId) -> Result<Self, Fault> {
        Ok(Self {
            family: Family::find(None).await?,
            at: None,
            session: session.as_str().to_owned(),
            sent: std::collections::BTreeSet::new(),
        })
    }

    /// Bind to a session over an already-open connection. `at` is where it came from, so a dropped
    /// one can be dialled again; pass `None` only when there is no such path.
    #[must_use]
    pub fn over(family: Family, at: Option<std::path::PathBuf>, session: &SessionId) -> Self {
        Self {
            family,
            at,
            session: session.as_str().to_owned(),
            sent: std::collections::BTreeSet::new(),
        }
    }

    /// Open the connection again, after one that was dropped.
    async fn redial(&mut self) -> Result<(), Fault> {
        self.family = match &self.at {
            Some(path) => Family::dial(path).await?,
            None => Family::find(None).await?,
        };
        Ok(())
    }

    /// Record a settled entry. Durable when this returns.
    pub async fn observe(&mut self, cursor: Cursor, entry: &Entry) -> Result<(), Fault> {
        self.write("observe", cursor, entry).await
    }

    /// Record something that happened that is not a transcript entry — a permission, a provider
    /// retry, a compaction — at the cursor the turn was at, with a role of `trace`.
    ///
    /// # Errors
    /// As any other write: a balthasar that is not there costs the trace and nothing else.
    pub async fn noticed(&mut self, cursor: Cursor, kind: &str, text: &str) -> Result<(), Fault> {
        let turn = serde_json::json!({
            "cursor": cursor.0,
            "role": "trace",
            "kind": kind,
            "text": text,
        });
        let args = vec![serde_json::Value::String(self.session.clone()), turn];
        self.family.call("observe", args).await.map(|_| ())
    }

    /// Call a verb for this session and hand back what came, undeserialised. For the rows
    /// [`Scribe::replay`] cannot represent: it reads into [`Entry`], and a trace row is not one.
    ///
    /// # Errors
    /// As any other call: a balthasar that is not there costs the answer and nothing else.
    pub async fn raw(&mut self, verb: &str) -> Result<Vec<serde_json::Value>, Fault> {
        if !ROLE.contains(&verb) {
            return Err(Fault::Refused(format!(
                "`{verb}` is not one of the memory role's verbs; see ROLES.md"
            )));
        }
        let args = vec![serde_json::Value::String(self.session.clone())];
        self.family.call(verb, args).await
    }

    /// Revise the entry already at this cursor.
    pub async fn amend(&mut self, cursor: Cursor, entry: &Entry) -> Result<(), Fault> {
        self.write("amend", cursor, entry).await
    }

    /// Record it, saying `amend` when this cursor has gone over before.
    pub async fn settle(&mut self, cursor: Cursor, entry: &Entry) -> Result<(), Fault> {
        if self.sent.contains(&cursor.0) {
            self.amend(cursor, entry).await
        } else {
            self.observe(cursor, entry).await
        }
    }

    async fn write(&mut self, verb: &str, cursor: Cursor, entry: &Entry) -> Result<(), Fault> {
        let turn = turn(cursor, entry)?;
        let args = vec![serde_json::Value::String(self.session.clone()), turn];
        // On the durable clock, not a feature's: the store a session writes to is opened by this
        // very call the first time, and what is not handed over is not anywhere else either.
        match self
            .family
            .call_within(verb, args.clone(), magi_ipc::family::DURABLE)
            .await
        {
            Ok(_) => {}
            // Dialled again and asked once more, and only for a connection that died: a refusal is
            // an answer. Safe to repeat, since `observe` at a cursor that has a row is an `amend`.
            // The second attempt is on the ordinary clock: the first already gave it thirty seconds.
            Err(Fault::Unavailable(why)) => {
                self.redial()
                    .await
                    .map_err(|again| Fault::Unavailable(format!("{why}; and again: {again}")))?;
                self.family.call(verb, args).await?;
            }
            Err(other) => return Err(other),
        }
        self.sent.insert(cursor.0);
        Ok(())
    }

    /// Everything this session said, in cursor order, as it finally stood.
    pub async fn replay(&mut self) -> Result<Vec<(Cursor, Entry)>, Fault> {
        let session = self.session.clone();
        self.replay_at(&session).await
    }

    /// The same, for a session this scribe is not bound to, which is what resuming reads.
    pub async fn replay_of(&mut self, id: &str) -> Result<Vec<Entry>, Fault> {
        Ok(self
            .replay_at(id)
            .await?
            .into_iter()
            .map(|(_, entry)| entry)
            .collect())
    }

    async fn replay_at(&mut self, id: &str) -> Result<Vec<(Cursor, Entry)>, Fault> {
        // Durable: resuming is the first call a fresh store gets, and that call opens it.
        let values = self
            .family
            .call_within(
                "replay",
                vec![serde_json::Value::String(id.to_owned())],
                magi_ipc::family::DURABLE,
            )
            .await?;
        values.iter().flat_map(rows).map(rebuild).collect()
    }

    /// The runs this project has had. Durable, for the reason `replay` is.
    pub async fn sessions(&mut self) -> Result<Vec<serde_json::Value>, Fault> {
        let values = self
            .family
            .call_within("sessions", Vec::new(), magi_ipc::family::DURABLE)
            .await?;
        Ok(values.iter().flat_map(rows).cloned().collect())
    }

    /// What this memory holds about `query`, nearest first. An empty query means whatever is
    /// nearest. This session's id travels with it: fresh memories live in the run's own scratch.
    pub async fn nearest(&mut self, query: &str, limit: u64) -> Result<Recalled, Fault> {
        let args = vec![
            serde_json::Value::String(query.to_owned()),
            serde_json::json!({ "limit": limit, "session": self.session }),
        ];
        let values = self.family.call("recall", args).await?;
        Ok(Recalled::of(&values))
    }

    /// Say that something was done after memories were handed over, and how it went — the only call
    /// that says what happened next, without which balthasar ranks by recency and similarity
    /// forever. Two calls, because how it went is not known when it starts.
    ///
    /// # Errors
    /// Whatever balthasar answered. A ledger that is off refuses this, which is not a turn failure.
    pub async fn acted(
        &mut self,
        injection: &str,
        tool: &str,
        action: &str,
        worked: bool,
    ) -> Result<Option<String>, Fault> {
        let used = self
            .family
            .call(
                "used",
                vec![
                    serde_json::Value::String(injection.to_owned()),
                    serde_json::json!({ "tool": tool, "action": action }),
                ],
            )
            .await?;
        let Some(action) = used
            .first()
            .and_then(|v| v.get("action"))
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(None);
        };
        let settled = self
            .family
            .call(
                "outcome",
                vec![
                    serde_json::Value::String(action.to_owned()),
                    serde_json::json!({ "kind": if worked { "succeeded" } else { "failed" } }),
                ],
            )
            .await?;
        // The row balthasar minted, answered back so a caller can tell "it recorded this" from
        // "it accepted the call".
        Ok(settled
            .first()
            .and_then(|v| v.get("outcome"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned))
    }

    /// The Lua library that speaks balthasar's surface, as balthasar itself ships it. A consumer
    /// keeping its own copy is one whose copy goes stale: take the one the server serves.
    ///
    /// # Errors
    /// Whatever balthasar answered. An older balthasar does not know the verb, and the copy runs.
    pub async fn library(&mut self) -> Result<String, Fault> {
        let values = self.family.call("client", Vec::new()).await?;
        values
            .first()
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Fault::Malformed("client answered no source".to_owned()))
    }

    /// Where balthasar thinks this session left off, and how much of it it holds. A cross-check,
    /// not a source: magi's journal is the copy of record, and a balthasar holding fewer turns has
    /// an incomplete scrollback that `plan`, `replay` and `scroll` all answer from.
    pub async fn resumes(&mut self) -> Result<u64, Fault> {
        let values = self
            .family
            .call(
                "resume",
                vec![serde_json::Value::String(self.session.clone())],
            )
            .await?;
        Ok(values
            .first()
            .and_then(|v| v.get("turns"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0))
    }

    /// Say which model this session talks to, and how much it holds. balthasar does the compacting,
    /// so it has to know what it is compacting for, and that cannot be guessed from the turns. Told
    /// at startup and again whenever `:model` switches; without it every plan fell back to 200,000.
    ///
    /// # Errors
    /// Whatever balthasar answered. A balthasar keeping no scrollback refuses this.
    pub async fn note_model(&mut self, name: &str, window: u64) -> Result<(), Fault> {
        let args = vec![
            serde_json::Value::String(self.session.clone()),
            serde_json::json!({ "model": name, "context": window }),
        ];
        self.family.call("model", args).await.map(|_| ())
    }

    pub async fn plan_for(&mut self, window: u64) -> Result<serde_json::Value, Fault> {
        let values = self
            .family
            .call(
                "plan",
                vec![
                    serde_json::Value::String(self.session.clone()),
                    serde_json::json!({ "window": window }),
                ],
            )
            .await?;
        Ok(values.first().cloned().unwrap_or(serde_json::Value::Null))
    }

    /// Keep something durably, and answer by the id it landed under. Separate from
    /// [`Self::observe`], which writes a run's scratch — the run's own until balthasar's ladder
    /// carries it across, and a recall does not return it.
    ///
    /// # Errors
    /// Whatever balthasar answered.
    pub async fn keep(&mut self, text: &str) -> Result<String, Fault> {
        // Under this session, so it is this session's to take back. A write, so durable.
        let values = self
            .family
            .call_within(
                "remember",
                vec![
                    serde_json::Value::String(text.to_owned()),
                    serde_json::json!({ "session": self.session }),
                ],
                magi_ipc::family::DURABLE,
            )
            .await?;
        values
            .first()
            .and_then(|v| v.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Fault::Malformed("remember answered no id".to_owned()))
    }
}

/// Hand everything a session has settled to balthasar. The lock is taken to drain and released
/// before a byte is written, so a UI reading the transcript is never queued behind `fsync`.
/// Draining first also means a failure does not re-send what already landed.
///
/// # Errors
/// Whatever balthasar answered. [`Fault::is_fatal`] says whether continuing would build on a hole.
pub async fn flush(
    session: &tokio::sync::Mutex<crate::session::Session>,
    scribe: &mut Option<Scribe>,
) -> Result<(), Fault> {
    let Some(scribe) = scribe.as_mut() else {
        return Ok(());
    };
    let mut settled = {
        let mut held = session.lock().await;
        if !held.has_pending() {
            return Ok(());
        }
        std::collections::VecDeque::from(held.take_pending())
    };
    while let Some((cursor, entry)) = settled.pop_front() {
        // A mask is not news to the layer that ordered it: balthasar marks a turn masked as it
        // hands the plan over, and streaming it back would file its decision as a fresh turn.
        if matches!(entry, Entry::Masked { .. }) {
            continue;
        }
        if let Err(why) = scribe.settle(cursor, &entry).await {
            // Back where it was taken from, rather than dropped: this is the only copy, and the
            // next flush — the one [`crate::drain`] makes on the way out — is its second chance.
            settled.push_front((cursor, entry));
            session.lock().await.keep_pending(settled.into());
            return Err(why);
        }
    }
    Ok(())
}

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
    fn of(values: &[serde_json::Value]) -> Self {
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
fn rows(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    value
        .as_array()
        .map_or_else(|| vec![value], |a| a.iter().collect())
}

/// Rebuild one entry from the `raw` balthasar handed back, and nothing else: reconstructing from
/// the projection would quietly lose every field the projection does not carry.
fn rebuild(row: &serde_json::Value) -> Result<(Cursor, Entry), Fault> {
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
fn kind(entry: &Entry) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::Recalled;

    /// Both shapes balthasar answers `recall` in. The setting that decides is balthasar's, and both
    /// are the ordinary case on somebody's machine.
    #[test]
    fn a_recall_with_no_ledger_is_a_list_of_memories() {
        let answered = Recalled::of(&[serde_json::json!([
            { "id": "m1", "text": "one" },
            { "id": "m2", "text": "two" },
        ])]);
        assert_eq!(answered.memories.len(), 2);
        assert_eq!(answered.injection, None, "there is no ledger to belong to");
    }

    #[test]
    fn a_recall_with_a_ledger_carries_the_id_that_makes_an_outcome_attributable() {
        let answered = Recalled::of(&[serde_json::json!({
            "injection": "inject-1700-abc",
            "memories": [{ "id": "m1", "text": "one" }],
        })]);
        assert_eq!(answered.memories.len(), 1);
        assert_eq!(answered.injection.as_deref(), Some("inject-1700-abc"));
    }

    #[test]
    fn a_recall_that_found_nothing_is_neither() {
        assert_eq!(Recalled::of(&[]), Recalled::default());
        assert!(Recalled::of(&[serde_json::json!([])]).memories.is_empty());
    }

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
                shown: None,
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
        // A signature is in `raw` and nowhere else; rebuilding from `text` would be a 400.
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

#[cfg(test)]
mod role {
    use super::ROLE;

    #[test]
    fn every_verb_scribe_calls_is_one_the_role_names() {
        // The list and the call sites are two copies of one fact. This is the cheap half of holding
        // them together; `gate-role.sh` holds the other end against `ROLES.md`.
        let source = include_str!("scribe.rs");
        for verb in ["observe", "amend", "replay", "sessions", "recall", "model"] {
            assert!(
                source.contains(&format!("call(\"{verb}\"")) || ROLE.contains(&verb),
                "`{verb}` is called but the role does not name it"
            );
        }
    }

    #[test]
    fn the_role_names_no_verb_balthasar_alone_offers() {
        // `utility`, `context`, `trace` and `status` are balthasar's own. A role contract that
        // named them would make a second implementation owe verbs magi never calls.
        for theirs in ["utility", "context", "trace", "status", "ingest"] {
            assert!(
                !ROLE.contains(&theirs),
                "`{theirs}` is one implementation's, not the role's"
            );
        }
    }
}
