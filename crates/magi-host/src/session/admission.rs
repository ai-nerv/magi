use super::Session;
use magi_proto::{AgentStatus, Entry};
use std::collections::VecDeque;

#[derive(Debug)]
pub(crate) enum Request {
    Opening(Entry),
    Declare,
    Grants(Vec<magi_proto::permit::Grant>),
}

#[derive(Default)]
pub(super) struct Admission {
    next: u64,
    active: Option<u64>,
    waiting: VecDeque<Request>,
    changing: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub(crate) struct Transition(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for Transition {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Session {
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

    pub(crate) fn admit(&mut self, request: Request) -> Result<Option<(u64, Request)>, String> {
        if self.busy() {
            if self.admission.waiting.len() >= 256 {
                return Err("session queue is full; this request was not accepted".into());
            }
            self.admission.waiting.push_back(request);
            return Ok(None);
        }
        Ok(Some(self.reserve(request)))
    }

    fn reserve(&mut self, request: Request) -> (u64, Request) {
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
        (self.admission.next, request)
    }

    pub(crate) fn finish(&mut self, owner: u64) -> Option<(u64, Request)> {
        if self.admission.active != Some(owner) {
            return None;
        }
        self.admission.active = None;
        match self.admission.waiting.pop_front() {
            Some(request) => Some(self.reserve(request)),
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
            Some(Request::Opening(Entry::From { .. }))
        ) {
            if let Some(Request::Opening(entry)) = self.admission.waiting.pop_front() {
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
            id: MessageId::new("queued"),
            text: text.into(),
            aside: String::new(),
        })
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
