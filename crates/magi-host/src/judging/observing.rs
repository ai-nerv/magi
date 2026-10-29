use crate::events::{self, Correlation, DelegationReason, Fact, PolicyDecision, PolicyOutcome};
use magi_proto::permit::Decision;

pub(super) fn decided(policy: PolicyDecision, decision: Decision) -> Decision {
    if let Some(id) = events::identity() {
        events::emit(|| {
            (
                Correlation {
                    request_id: Some(id),
                    ..Correlation::default()
                },
                Fact::Policy { decision: policy },
            )
        });
    }
    decision
}

fn outcome(decision: &Decision) -> PolicyOutcome {
    match decision {
        Decision::Allow { .. } => PolicyOutcome::Allowed,
        Decision::Deny => PolicyOutcome::Denied,
    }
}

pub(super) fn delegated(reason: DelegationReason, decision: Decision) -> Decision {
    decided(
        PolicyDecision::Delegated {
            reason,
            outcome: outcome(&decision),
        },
        decision,
    )
}

pub(super) fn model(decision: Decision) -> Decision {
    decided(
        PolicyDecision::Model {
            outcome: outcome(&decision),
        },
        decision,
    )
}
