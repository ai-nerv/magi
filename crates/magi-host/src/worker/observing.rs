use crate::events::{self, AuthorizationOutcome, Correlation, Fact};
use magi_tools::approve::Authorization;

pub(super) fn authorization(result: Authorization) {
    let Some(id) = events::identity() else {
        return;
    };
    events::emit(|| {
        let outcome = match result {
            Authorization::Ungated => AuthorizationOutcome::Ungated,
            Authorization::StandingGrant => AuthorizationOutcome::StandingGrant,
            Authorization::ApproverAllowed => AuthorizationOutcome::ApproverAllowed,
            Authorization::ApproverDenied => AuthorizationOutcome::ApproverDenied,
        };
        (
            Correlation {
                request_id: Some(id),
                ..Correlation::default()
            },
            Fact::Authorization { outcome },
        )
    });
}
