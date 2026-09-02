//! Asking, from where the tool is.
//!
//! A tool runs deep inside a turn, on a thread that is not async, and the person who can answer
//! it is on the other end of a socket. This is the seam between those two facts: a tool calls
//! [`crate::ops::Ops::allow`], which blocks until somebody has answered, and everything about how the
//! question travels lives on the far side of [`Approver`].
//!
//! Blocking on purpose. A tool that asked and carried on would be asking for a record rather
//! than a decision, and there is no useful thing to do with "I am about to delete this" after
//! the fact.

use magi_proto::permit::{Action, Decision};

/// Something that can put a question to a person and wait for the answer.
pub trait Approver: Send + Sync {
    /// Ask about `action`, and block until it is answered.
    ///
    /// A refusal is as valid an answer as a grant: the tool reports it and the model reads it,
    /// which is how a model learns that a thing is not on offer rather than that it is broken.
    fn ask(&self, tool: &str, action: &Action) -> Decision;
}

/// An approver that says yes to everything, for tests and for `--yes`.
///
/// Named rather than a bare `None`, so a place that means "no gate here" says so.
pub struct AllowAll;

impl Approver for AllowAll {
    fn ask(&self, _tool: &str, _action: &Action) -> Decision {
        Decision::Allow {
            scope: magi_proto::permit::Scope::Once,
            lifetime: magi_proto::permit::Lifetime::Session,
        }
    }
}

/// An approver that says no to everything.
///
/// What a daemon with no UI attached uses. A question nobody can see is not a question, and
/// answering it "yes" on their behalf is the failure this whole mechanism exists to prevent.
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
