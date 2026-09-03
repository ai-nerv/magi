//! One session: its transcript, its journal, and the log every consumer reads.

use magi_journal::{Journal, JournalError};
use magi_proto::{AgentStatus, Cursor, Entry, HarnessEvent, SessionId};
use std::path::Path;
use tokio::sync::broadcast;

/// Events buffered for a consumer that has fallen behind.
///
/// A slow UI is dropped and reconnects with its cursor rather than being spooled for
/// indefinitely. Tau declines to disconnect a lagging peer and accepts unbounded growth as the
/// cost; with a durable journal behind us, a reattach costs a replay and nothing is lost.
const BROADCAST_CAPACITY: usize = 1024;

/// A live session.
pub struct Session {
    cancel: crate::cancel::Cancel,
    /// Everything this session could switch to, for the picker.
    choices: Vec<magi_proto::ModelChoice>,
    /// How much reasoning is being asked for, as a level name.
    thinking: String,
    /// Which model answers here, when one is configured.
    ///
    /// Held by the session rather than looked up by the UI: a UI that read the configuration
    /// for itself would report whatever is configured *now*, which after an edit is not what
    /// the daemon is actually talking to.
    model: Option<magi_proto::ModelInfo>,
    journal: Journal,
    status: AgentStatus,
    events: broadcast::Sender<HarnessEvent>,
    /// Messages from other instances that arrived while a turn was running.
    ///
    /// **Nothing another instance says interrupts a turn.** A main with ten subagents would
    /// otherwise be answering the first one's question while the second, third and fourth
    /// arrive, and a session that is mid-thought is the worst moment to hand it somebody else's.
    /// So an arrival is held here and dealt with when the turn ends.
    ///
    /// Held rather than journalled on arrival, and that part is not politeness: committing a
    /// message between an assistant's tool call and its result puts a user turn inside an
    /// exchange, which is a conversation no provider accepts.
    waiting: Vec<Entry>,
    /// Entries settled here and not yet handed to balthasar, by cursor.
    ///
    /// Keyed rather than appended, so a message amended once per delta batch is one write at the
    /// end instead of one per batch. Drained by [`Self::take_pending`] under a short lock and
    /// written outside it: a socket round trip held here would block every UI read behind
    /// balthasar's `fsync`.
    pending: std::collections::BTreeMap<u64, Entry>,
}

impl Session {
    /// Open a session, restoring whatever its journal holds.
    pub fn open(path: &Path, id: SessionId, cwd: &str, now: u64) -> Result<Self, JournalError> {
        let journal = Journal::open(path, id, cwd, now)?;
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Self {
            journal,
            status: AgentStatus::Idle,
            cancel: crate::cancel::Cancel::default(),
            model: None,
            choices: Vec::new(),
            thinking: "off".to_owned(),
            events,
            waiting: Vec::new(),
            pending: std::collections::BTreeMap::new(),
        })
    }

    /// Open a session on what balthasar holds, keeping nothing on disk.
    ///
    /// The transcript still lives here — every read comes from it — but this session's copy of
    /// record is balthasar's, and the entries it starts with are the ones balthasar replayed.
    #[must_use]
    pub fn recorded(id: SessionId, entries: Vec<Entry>) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            journal: Journal::recorded(id, entries),
            status: AgentStatus::Idle,
            cancel: crate::cancel::Cancel::default(),
            model: None,
            choices: Vec::new(),
            thinking: "off".to_owned(),
            events,
            waiting: Vec::new(),
            pending: std::collections::BTreeMap::new(),
        }
    }

    /// Take up what balthasar holds for another session, keeping everyone attached.
    ///
    /// The counterpart to [`Self::resume`] for a store that is not a file. The journal is
    /// swapped rather than the `Session` replaced, for the same reason: the broadcast channel is
    /// what every attached UI holds, and building a new one would leave every subscriber quiet
    /// on a resume that looked like it worked.
    pub fn resume_recorded(&mut self, id: SessionId, entries: Vec<Entry>) {
        self.journal = Journal::recorded(id, entries);
        self.status = AgentStatus::Idle;
        self.pending.clear();
        let _ = self.events.send(self.snapshot(self.cursor()));
    }

    /// Whether this session's transcript is also being written to disk.
    #[must_use]
    pub fn is_kept(&self) -> bool {
        self.journal.is_kept()
    }

    /// Whether nothing is running, so something new may start.
    #[must_use]
    pub fn idle(&self) -> bool {
        matches!(self.status, AgentStatus::Idle)
    }

    /// Keep this until the session has finished what it is doing.
    pub fn hold(&mut self, entry: Entry) {
        self.waiting.push(entry);
    }

    /// Take everything that was held, in the order it arrived.
    ///
    /// Emptied by the taking, so the same message cannot be dealt with twice — two turns ending
    /// close together would otherwise both find it there.
    pub fn release(&mut self) -> Vec<Entry> {
        std::mem::take(&mut self.waiting)
    }

    /// Take what has settled since the last time, in cursor order.
    ///
    /// Cheap and synchronous on purpose: the caller drains here and does the writing after it
    /// has let the lock go.
    pub fn take_pending(&mut self) -> Vec<(Cursor, Entry)> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(cursor, entry)| (Cursor(cursor), entry))
            .collect()
    }

    /// Whether anything is waiting to be written out.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Put this session onto a different journal, keeping everyone attached to it.
    ///
    /// The journal is swapped rather than the `Session` replaced, because the broadcast channel
    /// is what every attached UI is holding: building a new `Session` would build a new channel,
    /// and every subscriber would go quiet on a resume that looked like it worked.
    ///
    /// Nothing is carried over. The transcript, the cursor and the status all belong to the
    /// journal, and a status left behind would have a fresh session claiming to be mid-turn.
    ///
    /// # Errors
    /// When the journal will not open, in which case this session is left on the one it had.
    pub fn resume(&mut self, path: &Path, cwd: &str, now: u64) -> Result<(), JournalError> {
        let journal = Journal::open(path, self.journal.session().clone(), cwd, now)?;
        self.journal = journal;
        self.status = AgentStatus::Idle;
        let _ = self.events.send(self.snapshot(self.cursor()));
        Ok(())
    }

    /// The interrupt this session's turns watch.
    ///
    /// Handed out rather than acted on here: the turn runs on another thread, and the
    /// session is the one thing both it and the connection task already share.
    #[must_use]
    pub fn cancel(&self) -> crate::cancel::Cancel {
        self.cancel.clone()
    }

    /// Say which model this session talks to.
    pub fn set_model(&mut self, model: Option<magi_proto::ModelInfo>) {
        self.model = model;
    }

    /// Say what it could switch to.
    pub fn set_choices(&mut self, choices: Vec<magi_proto::ModelChoice>) {
        self.choices = choices;
    }

    /// The model this session talks to, by name.
    #[must_use]
    pub fn model_name(&self) -> Option<String> {
        self.model.as_ref().map(|m| m.name.clone())
    }

    /// Say how much reasoning is being asked for.
    pub fn set_thinking(&mut self, level: String) {
        self.thinking = level;
    }

    /// How much reasoning is being asked for.
    #[must_use]
    pub fn thinking(&self) -> &str {
        &self.thinking
    }

    /// Every token this session has spent.
    ///
    /// Summed from the journal rather than counted as it goes, so a resumed session reports
    /// what it actually accrued instead of starting again from zero.
    #[must_use]
    pub fn usage(&self) -> magi_proto::Usage {
        self.entries()
            .iter()
            .fold(magi_proto::Usage::default(), |total, entry| match entry {
                Entry::Assistant { usage, .. } => magi_proto::Usage {
                    input: total.input + usage.input,
                    output: total.output + usage.output,
                    cache_read: total.cache_read + usage.cache_read,
                    cache_write: total.cache_write + usage.cache_write,
                },
                _ => total,
            })
    }

    /// A handle for publishing into this session from elsewhere.
    ///
    /// Handed out rather than reached through the lock, because the thing that needs it is a
    /// turn running on another thread — and that thread is usually the one *holding* the lock,
    /// so asking for it would deadlock or, with `try_lock`, silently drop the message.
    #[must_use]
    pub fn publisher(&self) -> broadcast::Sender<HarnessEvent> {
        self.events.clone()
    }

    /// Subscribe to everything published from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<HarnessEvent> {
        self.events.subscribe()
    }

    /// The session's identity.
    #[must_use]
    pub fn id(&self) -> &SessionId {
        self.journal.session()
    }

    /// What the agent is doing.
    #[must_use]
    pub fn status(&self) -> &AgentStatus {
        &self.status
    }

    /// The position of the last entry.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        self.journal.cursor()
    }

    /// The transcript.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        self.journal.entries()
    }

    /// The state a UI attaching at `from` needs before the live stream makes sense.
    ///
    /// Everything at or before `from` is history the UI has already seen, so it arrives as
    /// entries; the live stream carries only what follows. A cold attach passes
    /// [`Cursor::ZERO`] and gets nothing, because there is nothing it has seen.
    #[must_use]
    pub fn snapshot(&self, from: Cursor) -> HarnessEvent {
        let kept = usize::try_from(from.0).unwrap_or(usize::MAX);
        HarnessEvent::SessionSnapshot {
            cursor: from,
            session: self.id().clone(),
            entries: self.entries().iter().take(kept).cloned().collect(),
            status: self.status.clone(),
            model: self.model.clone(),
            choices: self.choices.clone(),
            thinking: self.thinking.clone(),
        }
    }

    /// Everything after `from`, as the events that would have produced it.
    ///
    /// A reattaching UI folds these onto its snapshot and reaches the same transcript a cold
    /// replay would, which is the property the attach tests pin down.
    #[must_use]
    pub fn replay(&self, from: Cursor) -> Vec<HarnessEvent> {
        let skip = usize::try_from(from.0).unwrap_or(usize::MAX);
        self.entries()
            .iter()
            .enumerate()
            .skip(skip)
            .flat_map(|(index, entry)| {
                let cursor = Cursor(index as u64 + 1);
                events_for(cursor, entry)
            })
            .collect()
    }

    /// Append an entry, publish it, and return where it landed.
    pub fn commit(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        let cursor = self.journal.append(entry.clone())?;
        for event in events_for(cursor, &entry) {
            // A send with no subscribers is not a failure: the daemon outlives its UIs.
            let _ = self.events.send(event);
        }
        self.pending.insert(cursor.0, entry);
        Ok(cursor)
    }

    /// Replace the last entry and publish what changed about it.
    ///
    /// What changed, not what it now is. Describing an entry from nothing is right for a cold
    /// replay and wrong here: it opens with a `started` event, and a UI that takes that at face
    /// value gets a second copy of a message it is already showing. It also reports the whole
    /// body as a delta, which a UI appending deltas would then show twice over.
    pub fn amend(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        let previous = self.journal.entries().last().cloned();
        let cursor = self.journal.amend(entry.clone())?;
        for event in amendment_events(cursor, previous.as_ref(), &entry) {
            let _ = self.events.send(event);
        }
        self.pending.insert(cursor.0, entry);
        Ok(cursor)
    }

    /// The same, for an entry that is no longer the last one.
    ///
    /// A round of tool calls commits every call before running any of them, so by the time a
    /// result arrives its entry has others after it. [`Session::amend`] replaces the *last*
    /// entry, so every result but the last landed on the wrong one and was then overwritten —
    /// leaving calls with `result: null` for the rest of the session, which is a call the model
    /// made and never got an answer to.
    ///
    /// # Errors
    /// When the write fails.
    pub fn amend_at(&mut self, cursor: Cursor, entry: Entry) -> Result<(), JournalError> {
        let at = usize::try_from(cursor.0).unwrap_or(0).saturating_sub(1);
        let previous = self.journal.entries().get(at).cloned();
        self.journal.amend_at(cursor, entry.clone())?;
        for event in amendment_events(cursor, previous.as_ref(), &entry) {
            let _ = self.events.send(event);
        }
        self.pending.insert(cursor.0, entry);
        Ok(())
    }

    /// The same, for a message that is still arriving: published, not written down.
    ///
    /// Every delta of an answer goes through here. [`Session::amend`] would be correct and
    /// unusable — it appends a whole record and flushes, so a thousand-token answer would write
    /// the message a thousand times, each copy longer than the last. The events are what a UI
    /// renders from, and they are free; the writing waits for the `amend` that ends the message.
    ///
    /// The transcript stays current either way, so a UI attaching mid-answer sees what has
    /// arrived rather than an empty message that fills in at the end.
    pub fn revise(&mut self, entry: Entry) {
        let previous = self.journal.entries().last().cloned();
        let cursor = self.cursor();
        self.journal.revise(entry.clone());
        for event in amendment_events(cursor, previous.as_ref(), &entry) {
            let _ = self.events.send(event);
        }
        // Queued like a commit, though nothing is written yet. The queue coalesces by cursor, so
        // this costs no extra write — and without it a flush landing between the entry's first
        // commit and its settling amendment records the empty message it started as. That is
        // what a spawned balthasar exposed: the timing changed, and a turn came back blank.
        self.pending.insert(cursor.0, entry);
    }

    /// Tell everyone which model is answering now.
    ///
    /// Its own event, because a UI learns the model from the snapshot it attached with and
    /// there is otherwise nothing to change its mind. Republishing the status does not do it:
    /// a status event carries a status and nothing else.
    pub fn announce_model(&mut self) {
        let _ = self.events.send(HarnessEvent::ModelChanged {
            cursor: self.cursor(),
            model: self.model.clone(),
        });
    }

    /// Change what the agent is doing and tell everyone.
    ///
    /// Status is not journalled: it describes the daemon right now, and a session restored
    /// tomorrow is idle whatever it was doing when the process died.
    pub fn set_status(&mut self, status: AgentStatus) {
        self.status = status.clone();
        let _ = self.events.send(HarnessEvent::StatusChanged {
            cursor: self.cursor(),
            status,
        });
    }
}

/// The events describing how `entry` differs from `previous`.
///
/// Only the change is published, because every subscriber is already showing the entry as
/// it was. A body grows by an increment; a tool call gains a result; nothing is started
/// twice.
fn amendment_events(cursor: Cursor, previous: Option<&Entry>, entry: &Entry) -> Vec<HarnessEvent> {
    match (previous, entry) {
        (
            Some(Entry::Assistant {
                text: before,
                thinking: thought,
                ..
            }),
            Entry::Assistant {
                id,
                text,
                thinking,
                stop_reason,
                error,
                usage,
                ..
            },
        ) => {
            // A message that is not an extension of itself has been *retracted*, not continued:
            // an attempt that streamed half an answer and then failed, whose retry starts from
            // nothing. A delta is an append, so describing this as one would leave both copies
            // on screen. Described in full instead, which begins the message again.
            if !text.starts_with(before) || !thinking.starts_with(thought) {
                return events_for(cursor, entry);
            }
            let mut out = Vec::new();
            let added = grown(before, text);
            let reasoned = grown(thought, thinking);
            if !added.is_empty() || !reasoned.is_empty() {
                out.push(HarnessEvent::AssistantDelta {
                    cursor,
                    id: id.clone(),
                    text: added,
                    thinking: reasoned,
                });
            }
            if let Some(stop_reason) = stop_reason {
                out.push(HarnessEvent::AssistantEnded {
                    cursor,
                    id: id.clone(),
                    stop_reason: *stop_reason,
                    error: error.clone(),
                    usage: *usage,
                });
            }
            out
        }
        (Some(Entry::Tool { .. }), Entry::Tool { id, result, .. }) => result
            .as_ref()
            .map(|result| HarnessEvent::ToolCallEnded {
                cursor,
                id: id.clone(),
                result: result.clone(),
            })
            .into_iter()
            .collect(),
        // An amendment that changed the kind of entry, or arrived with no entry under it,
        // is not an amendment. Describing it in full is the only honest answer.
        _ => events_for(cursor, entry),
    }
}

/// The part of `now` that was not already in `before`.
///
/// Falls back to the whole of `now` when it is not an extension, which happens when a turn
/// is rewritten rather than continued -- an aborted message keeping what arrived, say.
fn grown(before: &str, now: &str) -> String {
    now.strip_prefix(before).unwrap_or(now).to_owned()
}

/// The events that reconstruct one entry from nothing.
fn events_for(cursor: Cursor, entry: &Entry) -> Vec<HarnessEvent> {
    match entry {
        // The aside is deliberately not replayed: it is context for the model, the UI never
        // renders it, and putting it on the wire would send every attach a copy of something
        // nothing on that end reads.
        Entry::User { id, text, .. } => vec![HarnessEvent::UserMessage {
            cursor,
            id: id.clone(),
            text: text.clone(),
        }],
        Entry::Assistant {
            id,
            text,
            thinking,
            stop_reason,
            error,
            usage,
            ..
        } => {
            let mut out = vec![
                HarnessEvent::AssistantStarted {
                    cursor,
                    id: id.clone(),
                },
                HarnessEvent::AssistantDelta {
                    cursor,
                    id: id.clone(),
                    text: text.clone(),
                    thinking: thinking.clone(),
                },
            ];
            if let Some(stop_reason) = stop_reason {
                out.push(HarnessEvent::AssistantEnded {
                    cursor,
                    id: id.clone(),
                    stop_reason: *stop_reason,
                    error: error.clone(),
                    usage: *usage,
                });
            }
            out
        }
        // Never journalled, so never replayed. A UI makes its own and the session has none to
        // give: this arm exists because the type allows one, not because one arrives.
        Entry::Notice { .. } => Vec::new(),
        Entry::From {
            who,
            kin,
            sort,
            text,
        } => vec![HarnessEvent::MessageArrived {
            cursor,
            who: who.clone(),
            kin: kin.clone(),
            sort: sort.clone(),
            text: text.clone(),
        }],
        Entry::Branch { id, keeps } => vec![HarnessEvent::Branched {
            cursor,
            id: id.clone(),
            keeps: *keeps,
        }],
        Entry::Compaction {
            id,
            summary,
            replaces,
        } => vec![HarnessEvent::Compacted {
            cursor,
            id: id.clone(),
            summary: summary.clone(),
            replaces: *replaces,
        }],
        Entry::Tool {
            id,
            name,
            args,
            result,
            ..
        } => {
            let mut out = vec![HarnessEvent::ToolCallStarted {
                cursor,
                id: id.clone(),
                name: name.clone(),
                args: args.clone(),
            }];
            if let Some(result) = result {
                out.push(HarnessEvent::ToolCallEnded {
                    cursor,
                    id: id.clone(),
                    result: result.clone(),
                });
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::{MessageId, StopReason};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-session-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("s.jsonl")
    }

    fn session(name: &str) -> Session {
        Session::open(&temp(name), SessionId::new("s1"), "/tmp", 0).expect("open")
    }

    fn user(text: &str) -> Entry {
        Entry::User {
            id: MessageId::new(text),
            text: text.to_owned(),
            aside: String::new(),
        }
    }

    #[test]
    fn committing_publishes_to_subscribers() {
        let mut s = session("publish");
        let mut rx = s.subscribe();
        s.commit(user("hi")).expect("commit");
        let event = rx.try_recv().expect("an event");
        assert!(matches!(event, HarnessEvent::UserMessage { .. }));
    }

    #[test]
    fn a_cold_snapshot_carries_nothing() {
        let mut s = session("cold");
        s.commit(user("hi")).expect("commit");
        match s.snapshot(Cursor::ZERO) {
            HarnessEvent::SessionSnapshot { entries, .. } => assert!(entries.is_empty()),
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    #[test]
    fn a_resume_snapshot_carries_what_the_ui_already_saw() {
        let mut s = session("resume");
        s.commit(user("one")).expect("commit");
        s.commit(user("two")).expect("commit");
        match s.snapshot(Cursor(1)) {
            HarnessEvent::SessionSnapshot { entries, .. } => assert_eq!(entries.len(), 1),
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    #[test]
    fn replay_covers_only_what_follows_the_cursor() {
        let mut s = session("replay");
        s.commit(user("one")).expect("commit");
        s.commit(user("two")).expect("commit");
        let events = s.replay(Cursor(1));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].cursor(), Cursor(2));
    }

    #[test]
    fn an_unfinished_assistant_entry_replays_without_an_end_event() {
        let mut s = session("unfinished");
        s.commit(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "partial".into(),
            thinking: String::new(),
            stop_reason: None,
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        })
        .expect("commit");
        let events = s.replay(Cursor::ZERO);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, HarnessEvent::AssistantEnded { .. })),
            "a turn still in flight has not ended"
        );
    }

    #[test]
    fn a_finished_assistant_entry_replays_start_delta_and_end() {
        let mut s = session("finished");
        s.commit(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "done".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        })
        .expect("commit");
        assert_eq!(s.replay(Cursor::ZERO).len(), 3);
    }

    #[test]
    fn status_is_published_but_not_journalled() {
        let mut s = session("status");
        let mut rx = s.subscribe();
        s.set_status(AgentStatus::Working {
            label: "Thinking".into(),
        });
        assert!(matches!(
            rx.try_recv().expect("an event"),
            HarnessEvent::StatusChanged { .. }
        ));
        assert!(
            s.entries().is_empty(),
            "status never reaches the transcript"
        );
    }
}

#[cfg(test)]
mod waiting;

#[cfg(test)]
mod streaming;
