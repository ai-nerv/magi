//! What a tool is, and how one is dispatched. magi runs no tool of its own: the tools come from the
//! `tools` role's program (casper), spawned per call — see [`supplier`] and `ROLES.md`. The one
//! builtin, `spawn`, coordinates the agent tree rather than doing work on the machine.

pub mod approve;
pub mod bound;
pub mod builtin;
pub mod cancel;
pub mod command;
pub mod environ;
pub mod holding;
pub mod masking;
pub mod mcp;
pub mod ops;
pub mod permit;
pub mod process;
pub mod question;
pub mod reaching;
pub mod registry;
pub mod repair;
pub mod schema;
pub mod supplier;
pub mod watching;

pub use cancel::{Cancel, Uncancelled};
pub use ops::Ops;
pub use registry::{Prepared, Registry, Sending, Tool};
pub use watching::{Event, Watch, Watchers};

use serde::{Deserialize, Serialize};

/// What a tool produced. `content` is what the model reads and `shown` is what the person is drawn;
/// they are not the same content, and a tool with nothing to add leaves the second empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub content: String,
    /// Whether the tool failed. A tool that ran and reported a problem is still a result, not an
    /// error: the model needs to read what went wrong in order to do something about it.
    pub is_error: bool,
    /// What the person sees, when it is more than the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<magi_proto::tooling::Shown>,
}

impl Output {
    /// A successful result.
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            shown: None,
        }
    }

    /// A failure the model should read and react to.
    #[must_use]
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            shown: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_still_a_result() {
        let output = Output::error("no such file");
        assert!(output.is_error);
        assert_eq!(output.content, "no such file");
    }
}
