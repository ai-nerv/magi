//! What a tool is, and the three the floor is made of.
//!
//! A tool is a name, a schema, and something that runs. What that something *is* — Rust here,
//! a Lua function, or a process on the other end of a socket — is a property of its
//! declaration, not a different registry. The turn loop cannot tell them apart, and that is
//! the point: adding a way to reach a tool must not add a way to run one.
//!
//! Only `read`, `write` and `edit` live here. They are the floor: pure filesystem, already
//! behind [`ops::Ops`], and the things that must never be missing. `bash` is deliberately not
//! among them — it is the tool whose requirements justify a process boundary, so it is
//! declared as one in `config/tools/`.

pub mod approve;
pub mod bound;
pub mod builtin;
pub mod cancel;
pub mod casper;
pub mod command;
pub mod environ;
pub mod holding;
pub mod masking;
pub mod mcp;
pub mod ops;
pub mod permit;
pub mod process;
pub mod question;
pub mod registry;
pub mod repair;
pub mod schema;

pub use cancel::{Cancel, Uncancelled};
pub use ops::{Ops, Shell};
pub use registry::{Prepared, Registry, Sending, Tool, Watch};

use serde::{Deserialize, Serialize};

/// What a tool produced.
///
/// Two faces, and they are not the same content: `content` is what the model reads and `shown`
/// is what the person is drawn. A tool with nothing to add about how it should look leaves the
/// second empty, which is what every tool here does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    /// Text the model sees.
    pub content: String,
    /// Whether the tool failed.
    ///
    /// A tool that ran and reported a problem is still a result, not an error: the model needs
    /// to read what went wrong in order to do something about it.
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
