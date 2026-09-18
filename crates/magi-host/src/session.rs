//! One session: its transcript, its journal, and the log every consumer reads.

pub(crate) mod admission;
mod fitting;
mod resuming;

use fitting::{SNAPSHOT_BUDGET, newest_within};
use magi_journal::{Journal, JournalError};
use magi_proto::{AgentStatus, Cursor, Entry, HarnessEvent, SessionId};
use tokio::sync::{broadcast, watch};

/// Events buffered for a consumer that has fallen behind. A slow UI is dropped and reconnects with
/// its cursor rather than being spooled for indefinitely; a reattach costs a replay and loses nothing.
const BROADCAST_CAPACITY: usize = 1024;

pub struct Session {
    helpers: crate::settling::Tasks,
    admission: admission::Admission,
    cancel: crate::cancel::Cancel,
    choices: Vec<magi_proto::ModelChoice>,
    thinking: String,
    /// Which of the model's providers serves it, by routing tag; `None` leaves it to the router.
    provider: Option<String>,
    /// Which model answers here, when one is configured. Held by the session rather than looked up
    /// by the UI, which would report what is configured now rather than what the daemon is using.
    model: Option<magi_proto::ModelInfo>,
    journal: Journal,
    status: AgentStatus,
    events: broadcast::Sender<HarnessEvent>,
    /// The current status, as a last-value channel: an observer that must not count as an attached
    /// UI (a headless child reporting its own phase) reads this rather than subscribing to `events`,
    /// which is what "is anybody here to approve" counts.
    phase: watch::Sender<AgentStatus>,
    /// Entries awaiting persistence, keyed by cursor and acknowledged after successful writes.
    pending: std::collections::BTreeMap<u64, Entry>,
    /// What each model has cost this session, a finished turn counted once under the model that
    /// answered it; a last-value channel like `phase`, for whoever reports on this session.
    spent: watch::Sender<Vec<(String, magi_proto::Usage)>>,
    tallied: std::collections::BTreeMap<String, magi_proto::Usage>,
    counted: std::collections::HashSet<magi_proto::MessageId>,
    /// What each tool's supplier said about its result, by call id: the stub to send instead of it,
    /// how to get it back, whether it must stay. Beside the entry, because it is balthasar's input.
    hints: std::collections::BTreeMap<String, magi_proto::tooling::Hints>,
    /// The layout the last request was built from, for a request balthasar cannot answer.
    laid: Option<magi_proto::laying::Layout>,
    /// When the last turn ended, so balthasar can tell a quick follow-up from a return.
    rested: Option<std::time::Instant>,
    /// Helper jobs a layout handed out that nothing waited for: run once the turn is over.
    deferred: Vec<magi_proto::laying::Job>,
    /// What the current prompt's helper jobs have cost, for the ones that run after its turn.
    helpers_spent: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Whether the person has been told that nothing is being recorded, so it is said once.
    pub unrecorded: bool,
}

impl Session {
    /// Open a session on what balthasar holds — the only constructor. The file journal is gone:
    /// two stores is one store and a copy that goes stale. The transcript still lives here because
    /// every read comes from it, but it is a window; the record is balthasar's.
    #[must_use]
    pub fn recorded(id: SessionId, entries: Vec<Entry>) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (phase, _) = watch::channel(AgentStatus::Idle);
        let (spent, _) = watch::channel(Vec::new());
        Self {
            helpers: crate::settling::Tasks::default(),
            admission: admission::Admission::default(),
            spent,
            tallied: std::collections::BTreeMap::new(),
            counted: std::collections::HashSet::new(),
            journal: Journal::recorded(id, entries),
            status: AgentStatus::Idle,
            cancel: crate::cancel::Cancel::default(),
            model: None,
            choices: Vec::new(),
            thinking: "off".to_owned(),
            provider: None,
            events,
            phase,
            pending: std::collections::BTreeMap::new(),
            hints: std::collections::BTreeMap::new(),
            laid: None,
            rested: None,
            deferred: Vec::new(),
            helpers_spent: std::sync::Arc::default(),
            unrecorded: false,
        }
    }

    /// Keep helper jobs to run when the turn is over.
    pub fn defer(&mut self, jobs: Vec<magi_proto::laying::Job>) {
        self.deferred.extend(jobs);
    }

    /// Take the helper jobs kept for after the turn.
    pub fn take_deferred(&mut self) -> Vec<magi_proto::laying::Job> {
        std::mem::take(&mut self.deferred)
    }

    /// The current prompt's helper budget, shared with whoever charges it.
    #[must_use]
    pub fn helpers_spent(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        std::sync::Arc::clone(&self.helpers_spent)
    }

    /// A new prompt's helper budget.
    pub fn set_helpers_spent(&mut self, spent: std::sync::Arc<std::sync::atomic::AtomicU64>) {
        self.helpers_spent = spent;
    }

    /// Keep what a tool's supplier said about the result of call `id`.
    pub fn hint(&mut self, id: &str, hints: magi_proto::tooling::Hints) {
        if !hints.is_empty() {
            self.hints.insert(id.to_owned(), hints);
        }
    }

    /// What the supplier of call `id` said about its result; nothing when it said nothing.
    #[must_use]
    pub fn hints(&self, id: &str) -> magi_proto::tooling::Hints {
        self.hints.get(id).cloned().unwrap_or_default()
    }

    /// Remember the layout a request was built from.
    pub fn lay(&mut self, layout: magi_proto::laying::Layout) {
        self.laid = Some(layout);
    }

    /// The layout the last request was built from.
    #[must_use]
    pub fn laid(&self) -> Option<&magi_proto::laying::Layout> {
        self.laid.as_ref()
    }

    /// Say that a turn has ended.
    pub fn rest(&mut self) {
        self.rested = Some(std::time::Instant::now());
    }

    /// How long since a turn last ended, in whole seconds. `None` before the first.
    #[must_use]
    pub fn idle_for(&self) -> Option<u64> {
        self.rested.map(|at| at.elapsed().as_secs())
    }

    /// A last-value view of this session's status, for an observer that must not be counted as an
    /// attached UI — subscribing to `events` would make a gated tool wait on it to approve.
    #[must_use]
    pub fn phase_watch(&self) -> watch::Receiver<AgentStatus> {
        self.phase.subscribe()
    }

    /// Whether nothing is running, so something new may start.
    #[must_use]
    pub fn idle(&self) -> bool {
        !self.busy() && matches!(self.status, AgentStatus::Idle)
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
        // A resumed session's turns are counted once there is a model to count them under.
        for entry in self.journal.entries().to_vec() {
            self.tally(&entry);
        }
    }

    /// What each model has cost this session, as it changes.
    #[must_use]
    pub fn spent_watch(&self) -> watch::Receiver<Vec<(String, magi_proto::Usage)>> {
        self.spent.subscribe()
    }

    /// Count a finished turn under the model answering now, and once: a turn is amended many times
    /// as it streams, and only its ending carries what it cost.
    fn tally(&mut self, entry: &Entry) {
        let Entry::Assistant {
            id,
            stop_reason: Some(_),
            usage,
            ..
        } = entry
        else {
            return;
        };
        let Some(model) = self.model_name() else {
            return;
        };
        if usage.prompt_tokens() == 0 && usage.output == 0 && usage.cost_micros == 0 {
            return;
        }
        if !self.counted.insert(id.clone()) {
            return;
        }
        self.tallied.entry(model).or_default().add(*usage);
        self.spent.send_replace(
            self.tallied
                .iter()
                .map(|(name, used)| (name.clone(), *used))
                .collect(),
        );
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

    pub fn set_provider(&mut self, provider: Option<String>) {
        self.provider = provider;
    }

    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
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
                    cost_micros: total.cost_micros + usage.cost_micros,
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
    /// The newest entries that fit come back: a frame past the wire's limit is refused whole, which
    /// takes the connection and whatever the client was about to say with it.
    #[must_use]
    pub fn snapshot(&self, from: Cursor) -> HarnessEvent {
        let kept = self
            .entries()
            .iter()
            .enumerate()
            .take_while(|(i, _)| self.cursor_at(*i).is_some_and(|c| c <= from))
            .count();
        HarnessEvent::SessionSnapshot {
            cursor: from,
            session: self.id().clone(),
            entries: newest_within(self.entries().iter().take(kept), SNAPSHOT_BUDGET),
            status: self.status.clone(),
            model: self.model.clone(),
            choices: self.choices.clone(),
            thinking: self.thinking.clone(),
        }
    }

    /// Everything after `from`, as the events that would have produced it, for a reattaching UI.
    #[must_use]
    pub fn replay(&self, from: Cursor) -> Vec<HarnessEvent> {
        self.entries()
            .iter()
            .enumerate()
            .filter(|(i, _)| self.cursor_at(*i).is_some_and(|c| c > from))
            .flat_map(|(index, entry)| {
                let cursor = self.cursor_at(index).expect("journal cursor");
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
        self.tally(&entry);
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
        self.tally(&entry);
        self.pending.insert(cursor.0, entry);
        Ok(cursor)
    }

    /// The same, for an entry that is no longer the last one: a round commits every call before
    /// running any, so a result's entry has others after it by the time it arrives.
    ///
    /// # Errors
    /// When the write fails.
    pub fn amend_at(&mut self, cursor: Cursor, entry: Entry) -> Result<(), JournalError> {
        let previous = self.amendment_target(cursor, &entry)?;
        self.journal.amend_at(cursor, entry.clone())?;
        for event in amendment_events(cursor, Some(&previous), &entry) {
            let _ = self.events.send(event);
        }
        self.tally(&entry);
        self.pending.insert(cursor.0, entry);
        Ok(())
    }

    fn amendment_target(&self, cursor: Cursor, entry: &Entry) -> Result<Entry, JournalError> {
        let previous = self.position(cursor).and_then(|at| self.entries().get(at));
        let same = match (previous, entry) {
            (Some(Entry::Assistant { id: old, .. }), Entry::Assistant { id, .. }) => old == id,
            (Some(Entry::Tool { id: old, .. }), Entry::Tool { id, .. }) => old == id,
            _ => false,
        };
        previous.filter(|_| same).cloned().ok_or_else(|| {
            JournalError::Refused(format!("entry identity does not match cursor {}", cursor.0))
        })
    }

    /// Update an identified streaming entry without publishing its terminal event.
    pub fn revise_at(&mut self, cursor: Cursor, entry: Entry) -> Result<(), JournalError> {
        let previous = self.amendment_target(cursor, &entry)?;
        self.journal.amend_at(cursor, entry.clone())?;
        for event in amendment_events(cursor, Some(&previous), &entry) {
            if !matches!(event, HarnessEvent::AssistantEnded { .. }) {
                let _ = self.events.send(event);
            }
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
        let status = self.admitted_status(status);
        self.status = status.clone();
        // The last-value view first, so a headless observer sees the change even with no UI here.
        self.phase.send_replace(status.clone());
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
