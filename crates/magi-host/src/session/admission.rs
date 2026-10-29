use super::Session;
use super::observing::Trace;
use magi_proto::{AgentStatus, Entry};
use std::collections::VecDeque;

#[derive(Debug)]
pub(crate) enum Request {
    Opening(Entry),
    Tracked { request: u64, entry: Entry },
    Declare,
    Grants(Vec<magi_proto::permit::Grant>),
    Reports,
}

impl Request {
    pub(crate) fn tracked(self, tracking: Option<u64>) -> Self {
        match (self, tracking) {
            (Self::Opening(entry), Some(request)) => Self::Tracked { request, entry },
            (request, _) => request,
        }
    }

    pub(crate) fn entry(&self) -> Option<&Entry> {
        match self {
            Self::Opening(entry) | Self::Tracked { entry, .. } => Some(entry),
            _ => None,
        }
    }
}

/// The largest message a session takes: a quarter of a frame, so its record still fits in one.
pub(crate) const ENTRY_MOST: usize = crate::skills::snapshot::PROMPT_MOST;

#[derive(Default)]
pub(super) struct Admission {
    pub(super) reports: super::reporting::Reports,
    next: u64,
    active: Option<u64>,
    retrying_reports: bool,
    waiting: VecDeque<(Request, Trace)>,
    trace: Trace,
    changing: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub(crate) struct Transition(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Admission {
    pub(super) fn observing(&self, owner: u64) -> bool {
        self.active == Some(owner) && self.trace.enabled()
    }

    pub(super) fn observed(&mut self, outcome: crate::events::TurnOutcome) {
        self.trace.finish(outcome);
    }
}

impl Drop for Transition {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Session {
    pub(crate) fn event_turn(&self) -> Option<crate::events::Id> {
        self.admission.active.and(self.admission.trace.identity())
    }

    pub(super) fn admitted_status(&self, status: AgentStatus) -> AgentStatus {
        if self.busy() && matches!(status, AgentStatus::Idle) {
            AgentStatus::Working {
                label: "Settling".into(),
            }
        } else {
            status
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.admission.active.is_some()
            || self
                .admission
                .changing
                .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn begin_resume(&self) -> Result<Transition, String> {
        if self.busy() {
            return Err("session is busy; resume after queued work finishes".into());
        }
        self.admission
            .changing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Transition(std::sync::Arc::clone(&self.admission.changing)))
    }

    #[cfg(test)]
    pub(crate) fn admit(&mut self, request: Request) -> Result<Option<(u64, Request)>, String> {
        let trace = Trace::request(&request);
        self.admit_observed(request, trace)
    }

    pub(crate) fn admit_observed(
        &mut self,
        request: Request,
        mut trace: Trace,
    ) -> Result<Option<(u64, Request)>, String> {
        trace.admission();
        if let Some(entry) = request.entry() {
            crate::skills::snapshot::check(entry, magi_proto::Cursor(u64::MAX))?;
        }
        if let Some(entry) = request.entry()
            && super::resuming::weight(entry) > ENTRY_MOST
        {
            return Err(format!(
                "this message is {} bytes and a message may be at most {ENTRY_MOST}; it was not \
                 accepted",
                super::resuming::weight(entry)
            ));
        }
        if let Some(Entry::User { images, .. }) = request.entry() {
            magi_model::validate_images(images)?;
        }
        if self.busy() {
            if matches!(request, Request::Tracked { .. }) {
                return Err("session is busy; tracked prompt was not accepted".into());
            }
            if self.admission.waiting.len() >= 256 {
                return Err("session queue is full; this request was not accepted".into());
            }
            trace.admitted(true);
            self.admission.waiting.push_back((request, trace));
            return Ok(None);
        }
        trace.admitted(false);
        Ok(Some(self.reserve(request, trace)))
    }

    #[allow(clippy::expect_used)]
    fn reserve(&mut self, request: Request, mut trace: Trace) -> (u64, Request) {
        self.admission.retrying_reports = matches!(request, Request::Reports);
        self.admission.next = self
            .admission
            .next
            .checked_add(1)
            .expect("turn identity exhausted");
        self.admission.active = Some(self.admission.next);
        self.cancel = crate::cancel::Cancel::default();
        self.status = AgentStatus::Working {
            label: "Queued".into(),
        };
        self.phase.send_replace(self.status.clone());
        trace.start();
        self.admission.trace = trace;
        let _ = self.events.send(magi_proto::HarnessEvent::TurnStarted {
            owner: self.admission.next,
            request: match &request {
                Request::Tracked { request, .. } => Some(*request),
                _ => None,
            },
        });
        (self.admission.next, request)
    }

    pub(crate) fn finish(&mut self, owner: u64) -> Option<(u64, Request)> {
        if self.admission.active != Some(owner) {
            return None;
        }
        if self.cancel.is_requested()
            && !self.admission.retrying_reports
            && !self.pending_reports().is_empty()
            && !self
                .admission
                .waiting
                .iter()
                .any(|(request, _)| matches!(request, Request::Reports))
        {
            self.admission
                .waiting
                .push_back((Request::Reports, Trace::default()));
        }
        self.admission.active = None;
        self.admission.trace = Trace::default();
        let _ = self
            .events
            .send(magi_proto::HarnessEvent::TurnFinished { owner });
        match self.admission.waiting.pop_front() {
            Some((request, trace)) => Some(self.reserve(request, trace)),
            None => {
                self.set_status(AgentStatus::Idle);
                None
            }
        }
    }

    pub(crate) fn take_arrivals(&mut self) -> Vec<Entry> {
        let mut entries = Vec::new();
        while matches!(
            self.admission.waiting.front(),
            Some((Request::Opening(Entry::From { .. }), _))
        ) {
            if let Some((Request::Opening(entry), _)) = self.admission.waiting.pop_front() {
                entries.push(entry);
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::{MessageId, SessionId};

    fn prompt(text: &str) -> Request {
        Request::Opening(Entry::User {
            images: Vec::new(),
            id: MessageId::new("queued"),
            text: text.into(),
            aside: String::new(),
        })
    }

    #[test]
    fn tracked_prompt_busy_refusal_does_not_queue_or_steal_turn_boundaries() {
        let mut session = Session::recorded(SessionId::new("tracked"), Vec::new());
        let mut events = session.subscribe();
        let (first, _) = session
            .admit(prompt("background"))
            .expect("admit")
            .expect("start");
        assert!(
            matches!(events.try_recv().expect("boundary"), magi_proto::HarnessEvent::TurnStarted { owner, request: None } if owner == first)
        );
        assert!(
            session
                .admit(prompt("racing").tracked(Some(7)))
                .expect_err("busy")
                .contains("busy")
        );
        assert!(session.admit(prompt("queued")).expect("queue").is_none());
        let (second, _) = session.finish(first).expect("queued start");
        assert!(
            matches!(events.try_recv().expect("end"), magi_proto::HarnessEvent::TurnFinished { owner } if owner == first)
        );
        assert!(
            matches!(events.try_recv().expect("start"), magi_proto::HarnessEvent::TurnStarted { owner, request: None } if owner == second)
        );
        assert!(session.finish(second).is_none());
        while events.try_recv().is_ok() {}
        let (third, _) = session
            .admit(prompt("accepted").tracked(Some(8)))
            .expect("admit")
            .expect("start");
        assert!(
            matches!(events.try_recv().expect("tracked"), magi_proto::HarnessEvent::TurnStarted { owner, request: Some(8) } if owner == third)
        );
        assert!(session.finish(third).is_none());
    }

    #[test]
    fn reservations_own_cancellation_and_only_their_completion_advances_fifo() {
        let mut session = Session::recorded(SessionId::new("admission"), Vec::new());
        let (first, _) = session
            .admit(prompt("first"))
            .expect("admission succeeded")
            .expect("admission succeeded");
        let stopped = session.cancel();
        stopped.request();
        assert!(
            session
                .admit(prompt("second"))
                .expect("admission succeeded")
                .is_none()
        );
        assert!(
            session
                .admit(prompt("third"))
                .expect("admission succeeded")
                .is_none()
        );
        session.set_status(AgentStatus::Idle);
        assert!(!session.idle());
        assert!(matches!(session.status(), AgentStatus::Working { .. }));
        assert!(session.finish(first + 1).is_none());
        assert!(session.busy());
        let (second, Request::Opening(Entry::User { text, .. })) =
            session.finish(first).expect("admission succeeded")
        else {
            panic!("second prompt")
        };
        assert_eq!(text, "second");
        assert!(stopped.is_requested());
        assert!(!session.cancel().is_requested());
        assert!(session.finish(first).is_none());
        assert!(session.busy());
        let (third, Request::Opening(Entry::User { text, .. })) =
            session.finish(second).expect("admission succeeded")
        else {
            panic!("third prompt")
        };
        assert_eq!(text, "third");
        assert!(session.finish(third).is_none());
        assert!(session.idle());
    }

    #[test]
    fn a_message_too_large_to_record_is_refused_at_the_door() {
        let mut session = Session::recorded(SessionId::new("large"), Vec::new());
        let refused = session
            .admit(prompt(&"x".repeat(ENTRY_MOST + 1)))
            .expect_err("refused");
        assert!(refused.contains("at most"), "{refused}");
        assert!(!session.busy(), "nothing was reserved for it");
        assert!(session.admit(prompt(&"x".repeat(ENTRY_MOST))).is_ok());
    }

    #[test]
    fn full_queue_refuses_without_accepting_or_discarding_other_prompts() {
        let mut session = Session::recorded(SessionId::new("bounded"), Vec::new());
        let (mut owner, _) = session
            .admit(prompt("active"))
            .expect("admission succeeded")
            .expect("admission succeeded");
        for n in 0..256 {
            assert!(
                session
                    .admit(prompt(&n.to_string()))
                    .expect("admission succeeded")
                    .is_none()
            );
        }
        assert!(
            session
                .admit(prompt("refused"))
                .expect_err("full queue refused")
                .contains("not accepted")
        );
        for n in 0..256 {
            let (next, Request::Opening(Entry::User { text, .. })) =
                session.finish(owner).expect("admission succeeded")
            else {
                panic!("queued prompt")
            };
            assert_eq!(text, n.to_string());
            owner = next;
        }
        assert!(session.finish(owner).is_none());
        assert!(session.idle());
    }
}
