//! Giving a tool rows and letting it fill them: magi says how much space, forwards what the person
//! does, and blits back what comes out. A surface is a renderer, never an authority.

use magi_proto::tooling::Surface;
use magi_proto::wondering::{Answered, Wonder};

pub trait Holds: Send + Sync {
    /// Reserve the rows, run the surface, and block until it finishes. `None` is not a refusal.
    fn hold(&self, tool: &str, surface: &Surface, args: &serde_json::Value) -> Option<String>;
}

/// Something that can answer what a surface asks about the session, blocking like everything else.
pub trait Answers: Send + Sync {
    /// Say what `wonder` asks for, or why not. Never silence — see [`Answered::Refused`].
    fn answer(&self, wonder: Wonder, args: &serde_json::Value) -> Answered;
}

/// An answerer that knows nothing and refuses rather than inventing.
pub struct Incurious;

impl Answers for Incurious {
    fn answer(&self, wonder: Wonder, _args: &serde_json::Value) -> Answered {
        Answered::Refused {
            because: format!("nothing here can answer `{}`", wonder.verb()),
        }
    }
}

/// A holder with no screen behind it, which gives nothing. What `magi -p` uses.
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
