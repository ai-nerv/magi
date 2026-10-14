//! Handing the transcript to balthasar, beside [`crate::session::Session`] rather than inside it:
//! a socket round trip under the session lock would hold it across balthasar's `fsync` and freeze
//! every UI read. What travels is `raw`, the serialised record; the rest is a projection.

use magi_ipc::family::{Family, Fault};
use magi_proto::{Cursor, Entry, SessionId};

/// The one connection to balthasar, as the session shares it. Named here rather than beside either
/// of its users, which would put `worker` and `turn` in a cycle.
pub type Held = std::sync::Arc<tokio::sync::Mutex<Option<Scribe>>>;
mod flushing;
mod resuming;
mod shape;
pub use flushing::flush;
#[cfg(test)]
pub(crate) use flushing::said_about;
pub(crate) use flushing::told;
pub use shape::Recalled;
pub(crate) use shape::{cut, tokens};
use shape::{rebuild, rows, turn};

/// The program that filled the `memory` role before it was a role, and the default when nothing
/// names another.
pub const BALTHASAR: &str = "balthasar";

/// Every verb the memory role names, from `ROLES.md`. Here so [`Scribe::raw`] cannot reach past the
/// contract: an escape hatch taking any verb makes the contract advisory.
const ROLE: &[&str] = &[
    "observe",
    "replay",
    "amend",
    "recall",
    "remember",
    "forget",
    "why",
    "scroll",
    "used",
    "outcome",
    "model",
    "resume",
    "sessions",
    "plan",
    "layout",
    "applied",
    "overflowed",
    "jobs",
    "job_done",
    "notes",
    "note_open",
    "changes",
    "undo",
    "approve",
    "reject",
];

/// How long after a failed dial the next is tried, at first and at most.
const BACKOFF: (std::time::Duration, std::time::Duration) = (
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(30),
);

/// The widest text a stubbed entry keeps first; each try that still does not fit keeps a quarter.
const STUB_FIRST: usize = 1024 * 1024;

/// What travels with an entry that the entry cannot say about itself: the assistant message a tool
/// row belongs to (a group is kept, stubbed or summarised whole), and what its tool said about it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Beside {
    pub group: Option<u64>,
    pub hints: magi_proto::tooling::Hints,
}

pub struct Scribe {
    /// `None` while the memory layer is not answering; dialled again no sooner than `retry`.
    family: Option<Family>,
    /// Where to dial to get this connection back. balthasar can restart, and it drops a caller that
    /// has been quiet — which magi is between prompts — so without this the first drop ended
    /// recording for the session.
    at: std::path::PathBuf,
    retry: (std::time::Instant, std::time::Duration),
    session: String,
    /// Whose transcript this is: the session's own, or one agent's within a run it shares.
    transcript: String,
    /// Cursors already sent, so a second write says `amend` rather than `observe`.
    sent: std::collections::BTreeSet<u64>,
}

impl Scribe {
    /// Bind to this process's own memory layer, or the one `$MAGI_API_SOCKET` names.
    pub async fn find(session: &SessionId) -> Result<Self, Fault> {
        let family = Family::find(None).await?;
        let at = family.path().to_owned();
        Ok(Self::over(family, Some(at), session))
    }

    /// Bind to a session over an already-open connection. `at` is where it came from, so a dropped
    /// one can be dialled again; `None` takes the path the connection was made to.
    #[must_use]
    pub fn over(family: Family, at: Option<std::path::PathBuf>, session: &SessionId) -> Self {
        let at = at.unwrap_or_else(|| family.path().to_owned());
        let mut scribe = Self::unconnected(at, session);
        scribe.family = Some(family);
        scribe
    }

    /// Bind to a session whose memory layer did not answer yet; every call dials it again, backing
    /// off, until it does.
    #[must_use]
    pub fn unconnected(at: std::path::PathBuf, session: &SessionId) -> Self {
        Self {
            family: None,
            at,
            retry: (std::time::Instant::now(), BACKOFF.0),
            session: session.as_str().to_owned(),
            transcript: session.as_str().to_owned(),
            sent: std::collections::BTreeSet::new(),
        }
    }

    /// Record this agent's transcript under its own key while its memory stays the run's: agents
    /// of one run share a session, and one transcript for all of them let each overwrite the others.
    #[must_use]
    pub fn recording_as(mut self, key: String) -> Self {
        self.transcript = key;
        self
    }

    /// The connection, dialled again when it is down and the backoff has passed.
    async fn link(&mut self) -> Result<&mut Family, Fault> {
        if self.family.is_none() {
            let now = std::time::Instant::now();
            if now < self.retry.0 {
                return Err(Fault::Unavailable(format!(
                    "{} is not answering; tried again in {:?}",
                    self.at.display(),
                    self.retry.0 - now
                )));
            }
            match Family::dial(&self.at).await {
                Ok(open) => {
                    self.family = Some(open);
                    self.retry.1 = BACKOFF.0;
                }
                Err(why) => {
                    self.retry = (now + self.retry.1, (self.retry.1 * 2).min(BACKOFF.1));
                    return Err(why);
                }
            }
        }
        self.family
            .as_mut()
            .ok_or_else(|| Fault::Unavailable("not connected".into()))
    }

    /// One call on the durable clock, dialled again and asked once more when the connection died.
    /// A refusal is an answer and is not repeated; the second try is on the ordinary clock.
    async fn durable(
        &mut self,
        verb: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let first = self
            .link()
            .await?
            .call_within(verb, args.clone(), magi_ipc::family::DURABLE)
            .await;
        match first {
            Err(Fault::Unavailable(why)) => {
                self.family = None;
                self.retry.0 = std::time::Instant::now();
                self.link()
                    .await
                    .map_err(|again| Fault::Unavailable(format!("{why}; and again: {again}")))?
                    .call(verb, args)
                    .await
            }
            other => other,
        }
    }

    /// Record a settled entry. Durable when this returns.
    pub async fn observe(
        &mut self,
        cursor: Cursor,
        entry: &Entry,
        beside: &Beside,
    ) -> Result<(), Fault> {
        self.write("observe", cursor, entry, beside).await
    }

    /// Call a verb of the role for this session with one argument, and hand back its first answer.
    pub async fn ask(
        &mut self,
        verb: &str,
        arg: serde_json::Value,
    ) -> Result<serde_json::Value, Fault> {
        Ok(self
            .ask_all(verb, arg)
            .await?
            .into_iter()
            .next()
            .unwrap_or(serde_json::Value::Null))
    }

    /// The same, handing back every row of the answer.
    pub async fn ask_all(
        &mut self,
        verb: &str,
        arg: serde_json::Value,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        role_verb(verb)?;
        let args = vec![serde_json::Value::String(self.transcript.clone()), arg];
        self.link().await?.call(verb, args).await
    }

    /// How to lay out the next request. One that does not lay out refuses, and everything is sent.
    pub async fn layout(&mut self, asked: serde_json::Value) -> Result<serde_json::Value, Fault> {
        self.ask("layout", asked).await
    }

    /// Say that the provider took the request built from layout `id`, and what it counted.
    pub async fn applied(&mut self, id: &str, usage: magi_proto::Usage) -> Result<(), Fault> {
        let arg = serde_json::json!({ "id": id, "usage": {
            "input": usage.input, "cache_read": usage.cache_read,
            "cache_write": usage.cache_write, "output": usage.output,
        }});
        self.ask("applied", arg).await.map(|_| ())
    }

    /// Say that the provider refused layout `id` as too long, and get a tighter one.
    pub async fn overflowed(&mut self, id: &str, said: &str) -> Result<serde_json::Value, Fault> {
        self.ask("overflowed", serde_json::json!({ "id": id, "said": said }))
            .await
    }

    /// The helper jobs balthasar has waiting.
    pub async fn jobs(&mut self) -> Result<Vec<serde_json::Value>, Fault> {
        let rows = self.ask_all("jobs", serde_json::json!({})).await?;
        Ok(rows
            .into_iter()
            .flat_map(|row| match row {
                serde_json::Value::Array(list) => list,
                serde_json::Value::Object(ref map) if map.contains_key("jobs") => {
                    map["jobs"].as_array().cloned().unwrap_or_default()
                }
                one => vec![one],
            })
            .collect())
    }

    /// Hand back what a helper job came to.
    pub async fn job_done(&mut self, done: serde_json::Value) -> Result<(), Fault> {
        self.ask("job_done", done).await.map(|_| ())
    }

    /// Record something that happened that is not a transcript entry — a permission, a provider
    /// retry, a compaction — at the cursor the turn was at, with a role of `trace`. The memory
    /// layer keeps a trace apart from the entry at that cursor.
    ///
    /// # Errors
    /// As any other write: a balthasar that is not there costs the trace and nothing else.
    pub async fn noticed(&mut self, cursor: Cursor, kind: &str, text: &str) -> Result<(), Fault> {
        let mut text = text.to_owned();
        shape::cut(&mut text, STUB_FIRST);
        let turn = serde_json::json!({
            "run": self.session,
            "cursor": cursor.0,
            "role": "trace",
            "kind": kind,
            "text": text,
        });
        let args = vec![serde_json::Value::String(self.transcript.clone()), turn];
        self.link().await?.call("observe", args).await.map(|_| ())
    }

    /// Call a verb for this session and hand back what came, undeserialised. For the rows
    /// [`Scribe::replay`] cannot represent: it reads into [`Entry`], and a trace row is not one.
    ///
    /// # Errors
    /// As any other call: a balthasar that is not there costs the answer and nothing else.
    pub async fn raw(&mut self, verb: &str) -> Result<Vec<serde_json::Value>, Fault> {
        role_verb(verb)?;
        let args = vec![serde_json::Value::String(self.transcript.clone())];
        self.link().await?.call(verb, args).await
    }

    /// Revise the entry already at this cursor.
    pub async fn amend(
        &mut self,
        cursor: Cursor,
        entry: &Entry,
        beside: &Beside,
    ) -> Result<(), Fault> {
        self.write("amend", cursor, entry, beside).await
    }

    /// Record it, saying `amend` when this cursor has gone over before.
    pub async fn settle(
        &mut self,
        cursor: Cursor,
        entry: &Entry,
        beside: &Beside,
    ) -> Result<(), Fault> {
        if self.sent.contains(&cursor.0) {
            self.amend(cursor, entry, beside).await
        } else {
            self.observe(cursor, entry, beside).await
        }
    }

    async fn write(
        &mut self,
        verb: &str,
        cursor: Cursor,
        entry: &Entry,
        beside: &Beside,
    ) -> Result<(), Fault> {
        let args = self.fitted(verb, cursor, entry, beside)?;
        // On the durable clock, not a feature's: the store a session writes to is opened by this
        // very call the first time, and what is not handed over is not anywhere else either.
        match self.durable(verb, args.clone()).await {
            Ok(_) => {}
            // An `observe` whose reply was lost landed after all, and the entry has changed since:
            // the row is there, so this is an `amend`.
            Err(Fault::Refused(why)) if verb == "observe" && why.contains("already holds") => {
                self.durable("amend", args).await?;
            }
            Err(other) => return Err(other),
        }
        self.sent.insert(cursor.0);
        Ok(())
    }

    /// The call that records `entry`, with its texts cut down until it fits in one frame.
    fn fitted(
        &self,
        verb: &str,
        cursor: Cursor,
        entry: &Entry,
        beside: &Beside,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let args = |entry: &Entry| -> Result<Vec<serde_json::Value>, Fault> {
            let mut turn = turn(cursor, entry, beside)?;
            if let Some(fields) = turn.as_object_mut() {
                fields.insert(
                    "run".into(),
                    serde_json::Value::String(self.session.clone()),
                );
            }
            Ok(vec![
                serde_json::Value::String(self.transcript.clone()),
                turn,
            ])
        };
        let whole = args(entry)?;
        if magi_ipc::family::fits(verb, &whole) {
            return Ok(whole);
        }
        let mut keep = STUB_FIRST;
        while keep > 0 {
            let cut = args(&shape::stubbed(entry, keep))?;
            if magi_ipc::family::fits(verb, &cut) {
                magi_model::noted!("scribe: entry {} recorded cut to {keep} bytes", cursor.0);
                return Ok(cut);
            }
            keep /= 4;
        }
        Err(Fault::Failed(format!(
            "the entry at cursor {} does not fit in one frame even cut down",
            cursor.0
        )))
    }

    /// Everything this session said, in cursor order, as it finally stood.
    pub async fn replay(&mut self) -> Result<Vec<(Cursor, Entry)>, Fault> {
        let session = self.transcript.clone();
        self.replay_at(&session).await
    }

    /// The same, for a session this scribe is not bound to, which is what resuming reads.
    pub async fn replay_of(&mut self, id: &str) -> Result<Vec<(Cursor, Entry)>, Fault> {
        self.replay_at(id).await
    }

    /// The durable scratch run of a transcript, defaulting to its key without the extension.
    pub async fn run_of(&mut self, id: &str) -> Result<SessionId, Fault> {
        let values = match self
            .link()
            .await?
            .call_within(
                "resume",
                vec![serde_json::Value::String(id.to_owned())],
                magi_ipc::family::DURABLE,
            )
            .await
        {
            Ok(values) => values,
            Err(Fault::Refused(_)) => return Ok(SessionId::new(id)),
            Err(why) => return Err(why),
        };
        Ok(SessionId::new(
            values
                .first()
                .and_then(|value| value.get("run"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(id),
        ))
    }

    async fn replay_at(&mut self, id: &str) -> Result<Vec<(Cursor, Entry)>, Fault> {
        // A page at a time: a long run in one reply was past the memory layer's frame limit, and
        // the resume that asked for it started over from nothing. Durable: resuming is the first
        // call a fresh store gets, and that call opens it.
        let mut out = Vec::new();
        let mut from = 0_u64;
        loop {
            let values = self
                .link()
                .await?
                .call_within(
                    "replay",
                    vec![
                        serde_json::Value::String(id.to_owned()),
                        serde_json::json!({ "from": from, "bytes": REPLAY_PAGE }),
                    ],
                    magi_ipc::family::DURABLE,
                )
                .await?;
            let page: Vec<&serde_json::Value> = values.iter().flat_map(rows).collect();
            if page.is_empty() {
                break;
            }
            let page = page
                .into_iter()
                .map(rebuild)
                .collect::<Result<Vec<_>, _>>()?;
            // Non-paging peers repeat the complete, unchanged transcript.
            if from > 0 && page == out {
                break;
            }
            for (cursor, entry) in page {
                if cursor.0 == 0 || cursor.0 < from || cursor.0 == u64::MAX {
                    return Err(Fault::Malformed("invalid replay cursor ordering".into()));
                }
                from = cursor.0 + 1;
                out.push((cursor, entry));
            }
        }
        Ok(out)
    }

    /// The runs this project has had. Durable, for the reason `replay` is.
    pub async fn sessions(&mut self) -> Result<Vec<serde_json::Value>, Fault> {
        let values = self
            .link()
            .await?
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
        let values = self.link().await?.call("recall", args).await?;
        Ok(Recalled::of(&values))
    }

    /// The project's memories, rather than this run's: what a person browsing the store is asking,
    /// where [`Self::nearest`] asks what the turn should be told and scopes to the run's scratch.
    pub async fn browsing(
        &mut self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let args = vec![
            serde_json::Value::String(query.to_owned()),
            serde_json::json!({ "limit": limit }),
        ];
        let values = self.link().await?.call("recall", args).await?;
        Ok(Recalled::of(&values).memories)
    }

    /// What one memory rests on: how sure the layer is of it, and which sessions asserted it.
    pub async fn why(&mut self, id: &str) -> Result<Vec<serde_json::Value>, Fault> {
        let args = vec![serde_json::Value::String(id.to_owned())];
        self.link().await?.call("why", args).await
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
            .link()
            .await?
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
            .link()
            .await?
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
        let values = self.link().await?.call("client", Vec::new()).await?;
        values
            .first()
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Fault::Malformed("client answered no source".to_owned()))
    }

    /// Where balthasar thinks this session left off, and how much of it it holds. A cross-check,
    /// not a source: magi's journal is the copy of record, and a balthasar holding fewer turns has
    /// an incomplete scrollback that `layout`, `replay` and `scroll` all answer from.
    pub async fn resumes(&mut self) -> Result<u64, Fault> {
        let args = vec![serde_json::Value::String(self.transcript.clone())];
        let values = self.link().await?.call("resume", args).await?;
        Ok(values
            .first()
            .and_then(|v| v.get("turns"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0))
    }

    /// What a memory layer that does not lay out would do with a window this size.
    pub async fn plan_for(&mut self, window: u64) -> Result<serde_json::Value, Fault> {
        self.ask("plan", serde_json::json!({ "window": window }))
            .await
    }

    /// Say which model this session talks to, and how much it holds: told at startup and whenever
    /// `:model` switches, so the store knows which model produced a run.
    pub async fn note_model(&mut self, name: &str, window: u64) -> Result<(), Fault> {
        let args = vec![
            serde_json::Value::String(self.transcript.clone()),
            serde_json::json!({ "model": name, "context": window }),
        ];
        self.link().await?.call("model", args).await.map(|_| ())
    }

    /// Keep something durably, and answer by the id it landed under. Separate from
    /// [`Self::observe`], which writes a run's scratch — the run's own until balthasar's ladder
    /// carries it across, and a recall does not return it.
    ///
    /// # Errors
    /// Whatever balthasar answered.
    pub async fn keep(&mut self, text: &str) -> Result<String, Fault> {
        // Under this session, so it is this session's to take back. A write, so durable.
        let args = vec![
            serde_json::Value::String(text.to_owned()),
            serde_json::json!({ "session": self.session }),
        ];
        let values = self
            .link()
            .await?
            .call_within("remember", args, magi_ipc::family::DURABLE)
            .await?;
        values
            .first()
            .and_then(|v| v.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Fault::Malformed("remember answered no id".to_owned()))
    }
}

/// Refuse a verb the memory role does not name, before anything is sent.
fn role_verb(verb: &str) -> Result<(), Fault> {
    if ROLE.contains(&verb) {
        return Ok(());
    }
    Err(Fault::Refused(format!(
        "`{verb}` is not one of the memory role's verbs; see ROLES.md"
    )))
}

/// How much of a replay one reply may carry, well inside the memory layer's frame limit.
const REPLAY_PAGE: u64 = 4 * 1024 * 1024;

#[cfg(test)]
mod tests;

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
