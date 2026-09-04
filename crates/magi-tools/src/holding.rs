//! Giving a tool rows, and letting it fill them.
//!
//! The general form of [`crate::question`]. That one asks in a shape magi chose — a line of text
//! and a list of options — so a tool that wanted to ask differently could not. This one hands over
//! *space*: magi says how much, forwards what the person does, and blits back what comes out
//! without reading it. It cannot tell a permission prompt from a file picker from a game, which is
//! the point — the list of things that can appear there stops being a list anybody extends.
//!
//! The shape is the same as everywhere else in this crate and for the same reason: a tool runs
//! deep inside a turn on a thread that is not async, the person is on the other end of a socket,
//! and neither end can call the other. So [`Holds::hold`] blocks until the surface is finished.
//!
//! **A surface is a renderer, never an authority.** What comes back is the id its tenant drew, not
//! a decision. One that could return "allowed" would be a sibling granting itself a permission,
//! and the ledger every other tool goes through would be a suggestion.

use magi_proto::tooling::Surface;

/// Something that can give a tool rows and drive it until it is done.
pub trait Holds: Send + Sync {
    /// Reserve the rows, run the surface, and block until it finishes.
    ///
    /// The id of whatever was chosen, or `None` when the rows could not be given — because
    /// nobody is attached to draw them, or because the tenant ended without an answer. `None` is
    /// not a refusal: what it means is the tool's to decide.
    fn hold(&self, tool: &str, surface: &Surface, args: &serde_json::Value) -> Option<String>;
}

/// A holder with no screen behind it, which gives nothing.
///
/// What `magi -p` uses, and what tests use when the surface is not the thing under test. Named
/// rather than a bare `None` so a place that means "there is no screen here" says so.
pub struct Screenless;

impl Holds for Screenless {
    fn hold(&self, _tool: &str, _surface: &Surface, _args: &serde_json::Value) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_screen_fills_no_rows_rather_than_pretending_to() {
        // `magi -p` has no terminal and nobody watching. Answering on their behalf is the failure
        // the whole mechanism exists to prevent, and it matters most where nobody is looking.
        let surface = Surface {
            rows: 8,
            about: "the dinosaur game".to_owned(),
            tick: Some(60),
        };
        assert_eq!(
            Screenless.hold("dino", &surface, &serde_json::Value::Null),
            None
        );
    }
}
