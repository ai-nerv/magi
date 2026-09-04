//! Asking the person anything, from where the tool is.
//!
//! The general form of [`crate::approve`]. That one asks a *permission* question — a verb, a
//! scope, a lifetime — because permission is the question magi already knew how to ask. This one
//! carries whatever the tool wanted to ask, with the answers it wants to offer, which is what a
//! selection tool, a confirmation and a form all need and none of them could express in scopes.
//!
//! The shape is the same and for the same reasons: a tool runs deep inside a turn on a thread
//! that is not async, the person who can answer is on the other end of a socket, and neither end
//! can call the other. So [`Asks::ask`] blocks until somebody answers, and everything about how
//! the question travels lives on the far side of the trait.
//!
//! **Nobody attached means no answer.** A question nobody can see is not a question, and choosing
//! on their behalf is the failure the whole mechanism exists to prevent — so `None` comes back
//! and the tool decides what that means for it.

use magi_proto::tooling::Ask;

/// Something that can put a question to a person and wait for the answer.
pub trait Asks: Send + Sync {
    /// Ask, and block until it is answered.
    ///
    /// The id of the chosen option, or `None` when nobody answered — because nobody was there,
    /// or because they closed it. `None` is not a refusal: what a refusal *means* is the tool's
    /// to decide, and a shell that treats it as "do not run" and a picker that treats it as
    /// "pick nothing" are both right.
    fn ask(&self, tool: &str, ask: &Ask) -> Option<String>;
}

/// An asker nobody is behind, which answers nothing.
///
/// What a daemon with no UI attached uses, and what tests use when the question is not the thing
/// under test. Named rather than a bare `None`, so a place that means "nobody can answer here"
/// says so.
pub struct Unanswered;

impl Asks for Unanswered {
    fn ask(&self, _tool: &str, _ask: &Ask) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::tooling::Answer;

    #[test]
    fn nobody_attached_answers_nothing_rather_than_choosing() {
        // The failure this exists to prevent: a question answered on somebody's behalf, on
        // exactly the sessions where nobody is watching.
        let ask = Ask {
            question: "run it?".to_owned(),
            options: vec![Answer {
                id: "yes".to_owned(),
                label: "Yes".to_owned(),
                about: String::new(),
            }],
            detail: Vec::new(),
        };
        assert_eq!(Unanswered.ask("bash", &ask), None);
    }
}
