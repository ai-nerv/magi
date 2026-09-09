//! Asking, from where the tool is: a tool calls [`crate::ops::Ops::allow`] on a thread that is not
//! async, and blocks until somebody on the other end of a socket has answered.

use magi_proto::permit::{Action, Decision};

/// Something that can put a question to a person and wait for the answer.
pub trait Approver: Send + Sync {
    /// Ask about `action`, and block until it is answered. A refusal is as valid an answer as a grant.
    fn ask(&self, tool: &str, action: &Action) -> Decision;
}

/// An approver that says yes to everything, for tests and for `--yes`.
pub struct AllowAll;

impl Approver for AllowAll {
    fn ask(&self, _tool: &str, _action: &Action) -> Decision {
        Decision::Allow {
            scope: magi_proto::permit::Scope::Once,
            lifetime: magi_proto::permit::Lifetime::Session,
        }
    }
}

/// An approver that says no to everything: what a daemon with no UI attached uses.
pub struct DenyAll;

impl Approver for DenyAll {
    fn ask(&self, _tool: &str, _action: &Action) -> Decision {
        Decision::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_all_allows() {
        let action = Action::Read {
            path: "/x".to_owned(),
        };
        assert!(matches!(
            AllowAll.ask("read", &action),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn deny_all_denies() {
        // A daemon with nobody attached must not answer on their behalf.
        let action = Action::Run {
            command: "rm -rf /".to_owned(),
            program: "rm".to_owned(),
        };
        assert_eq!(DenyAll.ask("shell", &action), Decision::Deny);
    }
}
