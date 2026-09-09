//! One session: its transcript, its journal, and the log every consumer reads.

use magi_journal::{Journal, JournalError};
use magi_proto::{AgentStatus, Cursor, Entry, HarnessEvent, SessionId};
use tokio::sync::broadcast;

/// Events buffered for a consumer that has fallen behind. A slow UI is dropped and reconnects with
/// its cursor rather than being spooled for indefinitely; a reattach costs a replay and loses nothing.
const BROADCAST_CAPACITY: usize = 1024;

pub struct Session {
    cancel: crate::cancel::Cancel,
    choices: Vec<magi_proto::ModelChoice>,
    thinking: String,
    /// Which model answers here, when one is configured. Held by the session rather than looked up
    /// by the UI, which would report what is configured now rather than what the daemon is using.
    model: Option<magi_proto::ModelInfo>,
    journal: Journal,
    status: AgentStatus,
    events: broadcast::Sender<HarnessEvent>,
    /// Messages from other instances that arrived while a turn was running. Nothing another
    /// instance says interrupts a turn. Held rather than journalled on arrival: committing one
    /// between an assistant's tool call and its result is a conversation no provider accepts.
    waiting: Vec<Entry>,
    /// Entries settled here and not yet handed to balthasar, by cursor. Keyed rather than appended,
    /// so an amended message is one write; drained under a short lock and written outside it.
    pending: std::collections::BTreeMap<u64, Entry>,
}

impl Session {
    /// Open a session on what balthasar holds — the only constructor. The file journal is gone:
    /// two stores is one store and a copy that goes stale. The transcript still lives here because
    /// every read comes from it, but it is a window; the record is balthasar's.
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

    /// Take up what balthasar holds for another session, keeping everyone attached. The journal is
    /// swapped rather than the `Session` replaced: a new broadcast channel leaves every UI quiet.
    pub fn resume_recorded(&mut self, id: SessionId, entries: Vec<Entry>) {
        self.journal = Journal::recorded(id, entries);
        self.status = AgentStatus::Idle;
        self.pending.clear();
        let _ = self.events.send(self.snapshot(self.cursor()));
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

    /// Take everything that was held, in the order it arrived. Emptied by the taking, so two turns
    /// ending close together cannot both deal with the same message.
    pub fn release(&mut self) -> Vec<Entry> {
        std::mem::take(&mut self.waiting)
    }

    /// Take what has settled since the last time, in cursor order. Cheap and synchronous: the
    /// caller drains here and does the writing after it has let the lock go.
    pub fn take_pending(&mut self) -> Vec<(Cursor, Entry)> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(cursor, entry)| (Cursor(cursor), entry))
            .collect()
    }

    /// Put back what a flush could not hand over. A cursor that has settled again since keeps the
    /// newer entry: what is waiting to be written is the entry as it stands, not as it was taken.
    pub fn keep_pending(&mut self, unsent: Vec<(Cursor, Entry)>) {
        for (cursor, entry) in unsent {
            self.pending.entry(cursor.0).or_insert(entry);
        }
    }

    /// Whether anything is waiting to be written out.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The interrupt this session's turns watch, handed out because the turn runs on another thread.
    #[must_use]
    pub fn cancel(&self) -> crate::cancel::Cancel {
        self.cancel.clone()
    }

    pub fn set_model(&mut self, model: Option<magi_proto::ModelInfo>) {
        self.model = model;
    }

    pub fn set_choices(&mut self, choices: Vec<magi_proto::ModelChoice>) {
        self.choices = choices;
    }

    #[must_use]
    pub fn model_name(&self) -> Option<String> {
        self.model.as_ref().map(|m| m.name.clone())
    }

    /// The model this session talks to, and what it says about itself.
    #[must_use]
    pub fn model(&self) -> Option<magi_proto::ModelInfo> {
        self.model.clone()
    }

    pub fn set_thinking(&mut self, level: String) {
        self.thinking = level;
    }

    #[must_use]
    pub fn thinking(&self) -> &str {
        &self.thinking
    }

    /// Every token this session has spent, summed from the journal so a resumed session reports
    /// what it accrued rather than starting again from zero.
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

    /// A handle for publishing into this session from elsewhere. Handed out rather than reached
    /// through the lock, because the thread that needs it is usually the one holding the lock.
    #[must_use]
    pub fn publisher(&self) -> broadcast::Sender<HarnessEvent> {
        self.events.clone()
    }

    /// Subscribe to everything published from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<HarnessEvent> {
        self.events.subscribe()
    }

    #[must_use]
    pub fn id(&self) -> &SessionId {
        self.journal.session()
    }

    #[must_use]
    pub fn status(&self) -> &AgentStatus {
        &self.status
    }

    #[must_use]
    pub fn cursor(&self) -> Cursor {
        self.journal.cursor()
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        self.journal.entries()
    }

    /// The state a UI attaching at `from` needs before the live stream makes sense. Everything at
    /// or before `from` arrives as entries; a cold attach passes [`Cursor::ZERO`] and gets nothing.
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

    /// Everything after `from`, as the events that would have produced it, for a reattaching UI.
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

    /// Replace the last entry and publish what changed about it — what changed, not what it now is:
    /// a full description opens with a `started` event and reports the whole body as a delta.
    pub fn amend(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        let previous = self.journal.entries().last().cloned();
        let cursor = self.journal.amend(entry.clone())?;
        for event in amendment_events(cursor, previous.as_ref(), &entry) {
            let _ = self.events.send(event);
        }
        self.pending.insert(cursor.0, entry);
        Ok(cursor)
    }

    /// The same, for an entry that is no longer the last one: a round commits every call before
    /// running any, so a result's entry has others after it by the time it arrives.
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
    /// [`Session::amend`] appends a whole record and flushes, so a thousand-token answer would
    /// write the message a thousand times. The transcript stays current either way.
    pub fn revise(&mut self, entry: Entry) {
        let previous = self.journal.entries().last().cloned();
        let cursor = self.cursor();
        self.journal.revise(entry.clone());
        for event in amendment_events(cursor, previous.as_ref(), &entry) {
            // The ending is not published from the path that writes nothing. A revision updates
            // memory and does not touch the disk; `AssistantEnded` says the message is final, and
            // `magi -p` prints and exits on it. `amend` publishes it, after the write and flush.
            if matches!(event, HarnessEvent::AssistantEnded { .. }) {
                continue;
            }
            let _ = self.events.send(event);
        }
        // Queued like a commit, though nothing is written yet. Without it a flush landing between
        // the entry's commit and its settling amendment records the empty message it started as.
        self.pending.insert(cursor.0, entry);
    }

    /// Tell everyone which model is answering now. Its own event: a UI learns the model from the
    /// snapshot it attached with, and a status event carries a status and nothing else.
    pub fn announce_model(&mut self) {
        let _ = self.events.send(HarnessEvent::ModelChanged {
            cursor: self.cursor(),
            model: self.model.clone(),
        });
    }

    /// Change what the agent is doing and tell everyone. Status is not journalled: a session
    /// restored tomorrow is idle whatever it was doing when the process died.
    pub fn set_status(&mut self, status: AgentStatus) {
        self.status = status.clone();
        let _ = self.events.send(HarnessEvent::StatusChanged {
            cursor: self.cursor(),
            status,
        });
    }
}

/// The events describing how `entry` differs from `previous`. Only the change is published, because
/// every subscriber is already showing the entry as it was.
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
            // A message that is not an extension of itself has been retracted, not continued. A
            // delta is an append, so this is described in full instead, beginning the message again.
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
        // An amendment that changed the kind of entry is not one; describing it in full is honest.
        _ => events_for(cursor, entry),
    }
}

/// The part of `now` that was not already in `before`, or the whole of `now` when it is not an
/// extension — an aborted message keeping what arrived, say.
fn grown(before: &str, now: &str) -> String {
    now.strip_prefix(before).unwrap_or(now).to_owned()
}

/// The events that reconstruct one entry from nothing.
fn events_for(cursor: Cursor, entry: &Entry) -> Vec<HarnessEvent> {
    match entry {
        // The aside is deliberately not replayed: it is context for the model and no UI renders it.
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
        // Never journalled, so never replayed; this arm exists because the type allows one.
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
        // Nothing on the wire. A mask changes only what the provider is sent — the transcript still
        // shows what the tool said — so there is no decision an attached UI could act on or draw.
        Entry::Masked { .. } => Vec::new(),
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

    /// A session holding nothing, which is what balthasar replays for one that has not run.
    fn session(_name: &str) -> Session {
        Session::recorded(SessionId::new("s1"), Vec::new())
    }

    #[test]
    fn a_revision_never_announces_an_ending_it_has_not_written() {
        // `revise` writes nothing, and `AssistantEnded` tells a listener the message is final —
        // `magi -p` acts on it. Asserted here, because in one process the `amend` wins the race.
        let mut session = session("revise-ending");
        let mut live = session.subscribe();
        let id = MessageId::new("a1");
        let started = Entry::Assistant {
            id: id.clone(),
            text: "half".to_owned(),
            thinking: String::new(),
            stop_reason: None,
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        };
        session.commit(started).expect("commit");
        let Entry::Assistant { text, .. } = session.entries()[0].clone() else {
            panic!("an assistant entry");
        };
        assert_eq!(text, "half");
        while live.try_recv().is_ok() {}

        // A finished message, revised rather than amended.
        session.revise(Entry::Assistant {
            id,
            text: "half an answer".to_owned(),
            thinking: String::new(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        });

        let mut sawticks = (false, false);
        while let Ok(event) = live.try_recv() {
            match event {
                HarnessEvent::AssistantDelta { .. } => sawticks.0 = true,
                HarnessEvent::AssistantEnded { .. } => sawticks.1 = true,
                _ => {}
            }
        }
        assert!(sawticks.0, "the growth is still published");
        assert!(
            !sawticks.1,
            "a revision must not announce an ending: nothing has been written"
        );
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
