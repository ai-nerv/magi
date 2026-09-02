//! Putting a permission question to whoever is attached, from inside a turn.
//!
//! The awkward shape this solves: a tool runs on a blocking thread deep inside a turn, and the
//! only person who can answer it is on the other end of a socket being served by an async loop.
//! Neither end can call the other directly.
//!
//! So the question goes out as an event and the answer comes back on a channel. The tool blocks
//! on a plain [`std::sync::mpsc`] receiver — it is not async and must not become async for this
//! — and the command loop, which *is* async, drops the answer into it with a non-blocking send.
//!
//! **Nobody attached means no.** A question nobody can see is not a question, and answering it
//! on their behalf is the whole failure this mechanism exists to prevent.

use magi_proto::permit::{Action, Decision};
use magi_proto::{Cursor, HarnessEvent, ToolCallId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a question waits before it answers itself with a refusal.
///
/// Long, because somebody may be reading it. Bounded, because a turn that waits forever on a UI
/// that has gone is a daemon nothing can recover.
const PATIENCE: Duration = Duration::from_secs(300);

/// The questions currently outstanding.
#[derive(Default)]
pub struct Pending {
    waiting: Mutex<HashMap<ToolCallId, std::sync::mpsc::Sender<Decision>>>,
}

impl Pending {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver an answer to whoever is waiting for it.
    ///
    /// An id nobody is waiting on is dropped: the turn it belonged to is over, and acting on it
    /// would allow something nobody is watching.
    pub fn answer(&self, id: &ToolCallId, decision: Decision) {
        let Ok(mut waiting) = self.waiting.lock() else {
            return;
        };
        if let Some(sender) = waiting.remove(id) {
            let _ = sender.send(decision);
        }
    }

    /// Register a question and hand back the end to wait on.
    fn register(&self, id: ToolCallId) -> Option<std::sync::mpsc::Receiver<Decision>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.waiting.lock().ok()?.insert(id, sender);
        Some(receiver)
    }

    /// Forget a question, so a timed-out one does not sit in the map for the session.
    fn forget(&self, id: &ToolCallId) {
        if let Ok(mut waiting) = self.waiting.lock() {
            waiting.remove(id);
        }
    }
}

/// Asks by publishing an event, and waits on the channel.
pub struct Asker {
    pending: Arc<Pending>,
    publish: Box<dyn Fn(HarnessEvent) + Send + Sync>,
    cursor: Box<dyn Fn() -> Cursor + Send + Sync>,
    attached: Box<dyn Fn() -> bool + Send + Sync>,
    next: std::sync::atomic::AtomicU64,
}

impl Asker {
    /// An asker that publishes with `publish` and numbers its questions from zero.
    #[must_use]
    pub fn new(
        pending: Arc<Pending>,
        publish: Box<dyn Fn(HarnessEvent) + Send + Sync>,
        cursor: Box<dyn Fn() -> Cursor + Send + Sync>,
        attached: Box<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            pending,
            publish,
            cursor,
            attached,
            next: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl magi_tools::approve::Approver for Asker {
    fn ask(&self, tool: &str, action: &Action) -> Decision {
        if !(self.attached)() {
            // Nobody is looking. Saying yes here would make the gate a formality on exactly the
            // sessions nobody is watching, which are the ones it matters on.
            return Decision::Deny;
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("p{n}"));
        let Some(receiver) = self.pending.register(id.clone()) else {
            return Decision::Deny;
        };

        (self.publish)(HarnessEvent::PermissionAsked {
            cursor: (self.cursor)(),
            id: id.clone(),
            tool: tool.to_owned(),
            action: action.clone(),
            offers: magi_tools::permit::Ledger::offers(action),
        });

        let answer = receiver.recv_timeout(PATIENCE).unwrap_or(Decision::Deny);
        self.pending.forget(&id);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_tools::approve::Approver;

    fn asker(attached: bool) -> (Arc<Pending>, Asker, Arc<Mutex<Vec<HarnessEvent>>>) {
        let pending = Arc::new(Pending::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let asker = Asker::new(
            Arc::clone(&pending),
            Box::new(move |event| {
                if let Ok(mut seen) = kept.lock() {
                    seen.push(event);
                }
            }),
            Box::new(|| Cursor(1)),
            Box::new(move || attached),
        );
        (pending, asker, seen)
    }

    fn read() -> Action {
        Action::Read {
            path: "/etc/shadow".to_owned(),
        }
    }

    #[test]
    fn nobody_attached_is_a_refusal_and_asks_nothing() {
        // A question nobody can see is not a question.
        let (_, asker, seen) = asker(false);
        assert_eq!(asker.ask("read", &read()), Decision::Deny);
        assert!(
            seen.lock().expect("lock").is_empty(),
            "and it is not published"
        );
    }

    #[test]
    fn a_question_is_published_with_the_widths_it_can_be_answered_at() {
        let (pending, asker, seen) = asker(true);
        let answering = std::thread::spawn(move || {
            // Wait for it to register, then answer.
            for _ in 0..200 {
                let id = ToolCallId::new("p0");
                pending.answer(
                    &id,
                    Decision::Allow {
                        scope: magi_proto::permit::Scope::Once,
                        lifetime: magi_proto::permit::Lifetime::Session,
                    },
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let decision = asker.ask("read", &read());
        assert!(matches!(decision, Decision::Allow { .. }));

        let seen = seen.lock().expect("lock");
        let HarnessEvent::PermissionAsked { action, offers, .. } = seen.first().expect("published")
        else {
            panic!("wrong event");
        };
        assert_eq!(action, &read());
        assert!(offers.len() >= 2, "narrow and broad, not just yes");
        drop(seen);
        let _ = answering.join();
    }

    #[test]
    fn an_answer_nobody_is_waiting_for_is_dropped() {
        // The turn it belonged to is over; acting on it would allow something unwatched.
        let pending = Pending::new();
        pending.answer(&ToolCallId::new("gone"), Decision::Deny);
    }
}
