//! Sampling model, request context and outstanding questions.

use super::Session;
use magi_proto::HarnessEvent;
use magi_proto::monitoring::{AgentDetails, RequestContext};

impl Session {
    pub fn details(&self, (approvals, questions): (usize, usize)) -> AgentDetails {
        AgentDetails {
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            status: self.status.clone(),
            approvals,
            questions,
            context: self.request_context.clone(),
            cost_micros: self
                .tallied
                .values()
                .fold(0_u64, |sum, used| sum.saturating_add(used.cost_micros)),
        }
    }

    pub fn report_admission(&mut self, event: HarnessEvent) {
        if let HarnessEvent::RequestAdmitted {
            counted,
            capacity,
            limit,
            counting,
            outcome,
            ..
        } = &event
            && outcome == "admitted"
        {
            self.request_context = Some(RequestContext {
                input: *counted,
                capacity: *capacity,
                limit: *limit,
                estimated: counting != "verified",
            });
        }
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitoring_keeps_the_active_model_and_last_admitted_input() {
        let mut session = crate::open_session(1, "");
        assert!(session.details((0, 0)).context.is_none());
        session.set_model(Some(magi_proto::ModelInfo {
            name: "first".into(),
            context_window: 128000,
        }));
        session.set_thinking("high".into());
        let admitted = |outcome: &str, counted| HarnessEvent::RequestAdmitted {
            capacity: 128000,
            limit: 64000,
            reply: 1000,
            margin: 0,
            counted,
            counting: "estimated".into(),
            outcome: outcome.into(),
            why: String::new(),
        };
        session.report_admission(admitted("admitted", 24000));
        session.report_admission(admitted("refused", 90000));
        let details = session.details((2, 1));
        assert_eq!(details.model.expect("model").name, "first");
        assert_eq!(details.thinking, "high");
        assert_eq!((details.approvals, details.questions), (2, 1));
        let context = details.context.expect("context");
        assert_eq!(context.input, 24000);
        assert!(context.estimated);
        session.set_model(Some(magi_proto::ModelInfo {
            name: "second".into(),
            context_window: 64000,
        }));
        let changed = session.details((0, 0));
        assert_eq!(changed.model.expect("model").name, "second");
        assert!(changed.context.is_none());
    }
}
