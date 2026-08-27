//! One session: its transcript, its journal, and the log every consumer reads.

use axum_journal::{Journal, JournalError};
use axum_proto::{AgentStatus, Cursor, Entry, HarnessEvent, SessionId};
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
    journal: Journal,
    status: AgentStatus,
    events: broadcast::Sender<HarnessEvent>,
}

impl Session {
    /// Open a session, restoring whatever its journal holds.
    pub fn open(path: &Path, id: SessionId, cwd: &str, now: u64) -> Result<Self, JournalError> {
        let journal = Journal::open(path, id, cwd, now)?;
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Self {
            journal,
            status: AgentStatus::Idle,
            events,
        })
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
        Ok(cursor)
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

/// The events that reconstruct one entry.
fn events_for(cursor: Cursor, entry: &Entry) -> Vec<HarnessEvent> {
    match entry {
        Entry::User { id, text } => vec![HarnessEvent::UserMessage {
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
                });
            }
            out
        }
        Entry::Tool {
            id,
            name,
            args,
            result,
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
    use axum_proto::{MessageId, StopReason};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("axum-session-{}-{name}", std::process::id()));
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
