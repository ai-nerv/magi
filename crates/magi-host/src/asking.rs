//! Putting a permission question to whoever is attached, from inside a turn. A tool runs on a
//! blocking thread deep inside a turn and the only person who can answer is behind an async socket,
//! so the question goes out as an event and the answer comes back on a plain [`std::sync::mpsc`]
//! receiver. Nobody attached means no: a question nobody can see is not a question.

use magi_proto::permit::{Action, Decision};
use magi_proto::{Cursor, HarnessEvent, ToolCallId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a question waits before it answers itself with a refusal.
const PATIENCE: Duration = Duration::from_secs(300);

/// The questions currently outstanding. Two maps, because a permission comes back as a
/// [`Decision`] and a general question as the id of a chosen option.
#[derive(Default)]
pub struct Pending {
    waiting: Mutex<HashMap<ToolCallId, std::sync::mpsc::Sender<Decision>>>,
    choosing: Mutex<HashMap<ToolCallId, std::sync::mpsc::Sender<String>>>,
    /// Each open question as it was published, in the order asked. A question is an event and is
    /// never journalled, so this is the only place a screen that attaches later can learn of it.
    asked: Mutex<Vec<(ToolCallId, HarnessEvent)>>,
}

impl Pending {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every question still waiting for an answer, oldest first, for a screen that has just
    /// attached: without this a turn waits on a prompt nobody was ever shown.
    #[must_use]
    pub fn open(&self) -> Vec<HarnessEvent> {
        self.asked
            .lock()
            .map(|asked| asked.iter().map(|(_, event)| event.clone()).collect())
            .unwrap_or_default()
    }

    fn opened(&self, id: &ToolCallId, event: HarnessEvent) {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push((id.clone(), event));
        }
    }

    fn closed(&self, id: &ToolCallId) {
        if let Ok(mut asked) = self.asked.lock() {
            asked.retain(|(open, _)| open != id);
        }
    }

    /// Deliver an answer to whoever is waiting for it. An id nobody is waiting on is dropped: the
    /// turn it belonged to is over.
    pub fn answer(&self, id: &ToolCallId, decision: Decision) {
        self.closed(id);
        let Ok(mut waiting) = self.waiting.lock() else {
            return;
        };
        if let Some(sender) = waiting.remove(id) {
            let _ = sender.send(decision);
        }
    }

    /// Register a question and hand back the end to wait on.
    fn register(
        &self,
        id: ToolCallId,
        event: HarnessEvent,
    ) -> Option<std::sync::mpsc::Receiver<Decision>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.waiting.lock().ok()?.insert(id.clone(), sender);
        self.opened(&id, event);
        Some(receiver)
    }

    /// Deliver a chosen option to whoever is waiting for it; an id nobody waits on is dropped.
    pub fn chose(&self, id: &ToolCallId, choice: String) {
        self.closed(id);
        let Ok(mut choosing) = self.choosing.lock() else {
            return;
        };
        if let Some(sender) = choosing.remove(id) {
            let _ = sender.send(choice);
        }
    }

    /// Register a general question and hand back the end to wait on.
    fn awaiting(
        &self,
        id: ToolCallId,
        event: HarnessEvent,
    ) -> Option<std::sync::mpsc::Receiver<String>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.choosing.lock().ok()?.insert(id.clone(), sender);
        self.opened(&id, event);
        Some(receiver)
    }

    /// Forget a chosen-option question.
    fn drop_choice(&self, id: &ToolCallId) {
        self.closed(id);
        if let Ok(mut choosing) = self.choosing.lock() {
            choosing.remove(id);
        }
    }

    /// Forget a question, so a timed-out one does not sit in the map for the session.
    fn forget(&self, id: &ToolCallId) {
        self.closed(id);
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
            // Nobody is looking. Saying yes here would make the gate a formality on unwatched runs.
            return Decision::Deny;
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("p{n}"));
        let asked = HarnessEvent::PermissionAsked {
            cursor: (self.cursor)(),
            id: id.clone(),
            tool: tool.to_owned(),
            action: action.clone(),
            offers: magi_tools::permit::Ledger::offers(action),
        };
        let Some(receiver) = self.pending.register(id.clone(), asked.clone()) else {
            return Decision::Deny;
        };
        (self.publish)(asked);

        let answer = receiver.recv_timeout(PATIENCE).unwrap_or(Decision::Deny);
        self.pending.forget(&id);
        answer
    }
}

impl magi_tools::question::Asks for Asker {
    fn ask(&self, tool: &str, ask: &magi_proto::tooling::Ask) -> Option<String> {
        if !(self.attached)() {
            // Nobody is looking, so nobody can answer; choosing on their behalf is what this prevents.
            return None;
        }
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = ToolCallId::new(format!("q{n}"));
        let asked = HarnessEvent::Asked {
            cursor: (self.cursor)(),
            id: id.clone(),
            tool: tool.to_owned(),
            question: ask.question.clone(),
            options: ask.options.clone(),
            detail: ask.detail.clone(),
        };
        let receiver = self.pending.awaiting(id.clone(), asked.clone())?;
        (self.publish)(asked);

        // The same patience a permission gets. An unanswered question is not a refusal — the tool decides.
        let answer = receiver.recv_timeout(PATIENCE).ok();
        self.pending.drop_choice(&id);
        answer
    }
}

/// The three ways a turn reaches whoever is attached: a permission, a question, and rows a tool
/// draws in itself. They have never travelled apart.
#[derive(Clone)]
pub struct Person {
    pub approver: Arc<dyn magi_tools::approve::Approver>,
    pub asks: Arc<dyn magi_tools::question::Asks>,
    pub holds: Arc<dyn magi_tools::holding::Holds>,
    /// The surfaces currently on screen, so a keypress reaches the one holding the rows.
    pub surfaces: Arc<crate::holder::Holding>,
}

impl Person {
    /// Every face of one asker, plus the holder, which is not an asker: it spawns and pumps frames.
    #[must_use]
    pub fn of(
        asker: Arc<Asker>,
        holds: Arc<dyn magi_tools::holding::Holds>,
        surfaces: Arc<crate::holder::Holding>,
    ) -> Self {
        Self {
            approver: Arc::clone(&asker) as Arc<_>,
            asks: asker as Arc<_>,
            holds,
            surfaces,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_still_open_is_told_to_whoever_attaches_next() {
        // Asked once, as an event, and never journalled: a screen that stepped onto another agent
        // and came back was never told, and the turn waited out its five minutes unseen.
        let pending = Pending::new();
        let ask = |n: u8| HarnessEvent::Asked {
            cursor: Cursor::ZERO,
            id: ToolCallId::new(format!("q{n}")),
            tool: "read".into(),
            question: format!("question {n}"),
            options: Vec::new(),
            detail: Vec::new(),
        };
        let _first = pending
            .awaiting(ToolCallId::new("q1"), ask(1))
            .expect("registered");
        let _second = pending
            .awaiting(ToolCallId::new("q2"), ask(2))
            .expect("registered");
        assert_eq!(
            pending.open(),
            vec![ask(1), ask(2)],
            "both, in the order asked"
        );
        pending.chose(&ToolCallId::new("q1"), "yes".into());
        assert_eq!(
            pending.open(),
            vec![ask(2)],
            "an answered one is not asked again"
        );
        pending.drop_choice(&ToolCallId::new("q2"));
        assert!(pending.open().is_empty());
    }
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
