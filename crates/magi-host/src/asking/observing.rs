use super::WaitError;
use crate::events::{self, Correlation, Fact, Id, PermissionOutcome};
use magi_proto::permit::Decision;

pub(crate) struct Trace {
    id: Option<Id>,
    finished: bool,
}

impl Trace {
    pub(crate) fn new() -> Self {
        Self {
            id: events::identity(),
            finished: false,
        }
    }

    pub(crate) fn transition(&self, outcome: PermissionOutcome) {
        if let Some(id) = self.id {
            events::emit(|| {
                (
                    Correlation {
                        request_id: Some(id),
                        ..Correlation::default()
                    },
                    Fact::Permission { outcome },
                )
            });
        }
    }

    pub(crate) fn finish(&mut self, outcome: PermissionOutcome) {
        if !self.finished {
            self.finished = true;
            self.transition(outcome);
        }
    }

    pub(crate) fn result(&mut self, answer: &Result<Decision, WaitError>) {
        self.finish(match answer {
            Ok(Decision::Allow { .. }) => PermissionOutcome::Allowed,
            Ok(Decision::Deny) => PermissionOutcome::Denied,
            Err(WaitError::Cancelled) => PermissionOutcome::Cancelled,
            Err(WaitError::Expired) => PermissionOutcome::Expired,
            Err(WaitError::Unavailable) => PermissionOutcome::Unavailable,
        });
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        self.finish(PermissionOutcome::Unknown);
    }
}

#[cfg(test)]
mod tests;
