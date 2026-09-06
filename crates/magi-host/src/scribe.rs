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

/// The one connection to balthasar, as the session shares it.
///
/// Here rather than beside either of its users. It is held by the turn loop, by the surface's
/// question path and by the flush on the way out, and putting the name in any one of them made
/// that one a dependency of the others — `worker` and `turn` in a circle, which the cycle gate
/// said so about within the minute.
pub type Held = std::sync::Arc<tokio::sync::Mutex<Option<Scribe>>>;

/// A connection to balthasar, bound to one session.
pub struct Scribe {
    family: Family,
    /// Where to dial to get this connection back.
    ///
    /// A held connection is not a permanent one: balthasar can restart, and it drops a caller
    /// that has been quiet — which, between one prompt and the next, magi always is. Without a
    /// way back, the first such drop ended recording for the rest of the session and every turn
    /// after it was written into the transcript as lost.
    ///
    /// `None` when the connection was found rather than named, which is the answer for a
    /// balthasar magi did not start: the way back is to look again.
    at: Option<std::path::PathBuf>,
    session: String,
    /// Cursors already sent, so a second write says `amend` rather than `observe`.
    ///
    /// The two land in the same place and the second wins either way, but which verb was meant
    /// is not balthasar's to infer from whether a row happened to exist.
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

    /// Bind to a session over an already-open connection.
    ///
    /// `at` is where that connection came from, so a dropped one can be dialled again. Pass
    /// `None` only when there is no such path.
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
        match self.family.call(verb, args.clone()).await {
            Ok(_) => {}
            // Dialled again and asked once more, and only for a connection that died: a
            // refusal or a failed write is an answer, and asking twice would either repeat a
            // refusal or record a turn twice. Once, because a second failure is an outage
            // rather than a dropped handle, and a turn is not the place to sit retrying.
            //
            // Safe to repeat because the verb is addressed to a cursor: `observe` at a cursor
            // that already has a row is what `amend` means, so the worst a retry can do is
            // write what was already there.
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

    /// The same, for a session this scribe is not bound to.
    ///
    /// Resuming reads somebody else's run before becoming it, so the id is the argument rather
    /// than the one this connection was opened with.
    pub async fn replay_of(&mut self, id: &str) -> Result<Vec<Entry>, Fault> {
        Ok(self
            .replay_at(id)
            .await?
            .into_iter()
            .map(|(_, entry)| entry)
            .collect())
    }

    async fn replay_at(&mut self, id: &str) -> Result<Vec<(Cursor, Entry)>, Fault> {
        let values = self
            .family
            .call("replay", vec![serde_json::Value::String(id.to_owned())])
            .await?;
        values.iter().flat_map(rows).map(rebuild).collect()
    }

    /// The runs this project has had.
    pub async fn sessions(&mut self) -> Result<Vec<serde_json::Value>, Fault> {
        let values = self.family.call("sessions", Vec::new()).await?;
        Ok(values.iter().flat_map(rows).cloned().collect())
    }

    /// What this memory holds about `query`, nearest first.
    ///
    /// An empty query is what balthasar takes to mean "whatever is nearest", which is the answer
    /// to "what does this session remember" — so it is passed through rather than refused here.
    /// This session's own id travels with it, or a run could not find what it was told a minute
    /// ago: freshly written memories live in the run's own scratch until they are distilled.
    pub async fn nearest(&mut self, query: &str, limit: u64) -> Result<Recalled, Fault> {
        let args = vec![
            serde_json::Value::String(query.to_owned()),
            serde_json::json!({ "limit": limit, "session": self.session }),
        ];
        let values = self.family.call("recall", args).await?;
        Ok(Recalled::of(&values))
    }

    /// Say that something was done after memories were handed over, and how it went.
    ///
    /// **The loop that decides whether a memory was any good.** Everything else here is one
    /// direction: the transcript goes over, memories come back. This is the only call that says
    /// what happened *next*, and without it balthasar can rank by recency and similarity and
    /// never by whether anything it offered was worth offering.
    ///
    /// magi reports the action and nothing more. Whether it followed from any particular memory
    /// is balthasar's to decide against the injection it served them under — a harness claiming
    /// a match it did not verify would be asserting an analysis rather than reporting an event.
    ///
    /// Two calls because they are two facts, and the second is not known when the first is: a
    /// tool that has been started has been used, and how it went is decided later.
    ///
    /// Answers the outcome row balthasar wrote, or `None` when it keeps no ledger.
    ///
    /// # Errors
    /// Whatever balthasar answered. A ledger that is off refuses this, which is not a failure of
    /// the turn.
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
        // "it accepted the call". They are the same reply otherwise, and the difference is the
        // whole of whether this loop is closed.
        Ok(settled
            .first()
            .and_then(|v| v.get("outcome"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned))
    }

    /// What balthasar has attributed to one memory: outcomes, and how often it was returned.
    ///
    /// Read-only, and the only way to see from out here that [`Self::acted`] landed rather than
    /// merely returned. Answers nothing useful when the ledger is off, which is the default.
    ///
    /// # Errors
    /// Whatever balthasar answered.
    pub async fn utility(&mut self, memory: &str) -> Result<serde_json::Value, Fault> {
        let values = self
            .family
            .call(
                "utility",
                vec![serde_json::Value::String(memory.to_owned())],
            )
            .await?;
        Ok(values.first().cloned().unwrap_or(serde_json::Value::Null))
    }

    /// The Lua library that speaks balthasar's surface, as balthasar itself ships it.
    ///
    /// **A consumer keeping its own copy is a consumer whose copy goes stale**, and this one did:
    /// magi's copy predated a fix to the connect path, so every session on that machine silently
    /// had no memory tools and nothing anywhere said why. The chicken and egg is real — a client
    /// is needed to ask for the client — and the answer is the one melchior already uses: connect
    /// with the copy you have, then take the one the server serves.
    ///
    /// # Errors
    /// Whatever balthasar answered. An older balthasar does not know the verb, which is not a
    /// failure: the bundled copy is then what runs, exactly as before.
    pub async fn library(&mut self) -> Result<String, Fault> {
        let values = self.family.call("client", Vec::new()).await?;
        values
            .first()
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Fault::Malformed("client answered no source".to_owned()))
    }

    /// Keep something durably, and answer by the id it landed under.
    ///
    /// Deliberately separate from [`Self::observe`], which writes a run's *scratch* — that is
    /// the run's own until something on balthasar's ladder carries it across, and a recall does
    /// not return it. What a turn is shown unasked should be established rather than the last
    /// thing anybody said.
    ///
    /// # Errors
    /// Whatever balthasar answered.
    pub async fn keep(&mut self, text: &str) -> Result<String, Fault> {
        // Under this session, so it is this session's to take back. A memory kept with no
        // session belongs to the project, and balthasar rightly refuses to let a peer forget
        // what it did not write.
        let values = self
            .family
            .call(
                "remember",
                vec![
                    serde_json::Value::String(text.to_owned()),
                    serde_json::json!({ "session": self.session }),
                ],
            )
            .await?;
        values
            .first()
            .and_then(|v| v.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Fault::Malformed("remember answered no id".to_owned()))
    }

    /// Stop asserting one memory.
    ///
    /// # Errors
    /// Whatever balthasar answered.
    pub async fn drop_memory(&mut self, id: &str) -> Result<(), Fault> {
        self.family
            .call("forget", vec![serde_json::Value::String(id.to_owned())])
            .await?;
        Ok(())
    }
}

/// Hand everything a session has settled to balthasar.
///
/// The lock is taken to drain and released before a byte is written, so a UI reading the
/// transcript is never queued behind balthasar's `fsync`. Draining first also means a failure
/// does not re-send what already landed: the queue is empty either way, and the error says the
/// turn may not continue.
///
/// # Errors
/// Whatever balthasar answered. [`Fault::is_fatal`] says whether continuing would build on a
/// hole.
pub async fn flush(
    session: &tokio::sync::Mutex<crate::session::Session>,
    scribe: &mut Option<Scribe>,
) -> Result<(), Fault> {
    let Some(scribe) = scribe.as_mut() else {
        return Ok(());
    };
    let settled = {
        let mut held = session.lock().await;
        if !held.has_pending() {
            return Ok(());
        }
        held.take_pending()
    };
    for (cursor, entry) in settled {
        scribe.settle(cursor, &entry).await?;
    }
    Ok(())
}

/// What a recall answered, and the ledger entry it belongs to.
///
/// **balthasar answers `recall` in two shapes**, and which one depends on a setting magi does not
/// hold. With its ledger off it hands back a bare list of memories; with the ledger on it hands
/// back `{ injection, memories }`, because handing memories to something that is about to put
/// them in a model's context *is* an injection and the id is what makes an outcome attributable
/// to it later.
///
/// Read here rather than at the call site so there is one place that knows both shapes. Reading
/// only the first is how the automatic path came to carry no injection id at all, which left
/// `used` and `outcome` — the loop that decides whether a memory was any good — with nothing to
/// report against.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Recalled {
    /// The memories, in the order balthasar ranked them.
    pub memories: Vec<serde_json::Value>,
    /// The ledger entry these were served under, when balthasar is keeping one.
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
    use super::Recalled;

    /// Both shapes balthasar answers `recall` in, and which is which.
    ///
    /// The setting that decides is balthasar's, not magi's, so this cannot be settled by
    /// convention: a magi that read only the bare-list shape saw the ledger form as one
    /// unparseable memory, and one that read only the wrapper saw nothing at all when the ledger
    /// was off. Both are the ordinary case on somebody's machine.
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
