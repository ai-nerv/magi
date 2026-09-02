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
pub mod command;
pub mod environ;
pub mod ops;
pub mod permit;
pub mod process;
pub mod registry;
pub mod repair;
pub mod schema;

pub use cancel::{Cancel, Uncancelled};
pub use ops::{Ops, Shell};
pub use registry::{Prepared, Registry, Sending, Tool, Watch};

use serde::{Deserialize, Serialize};

/// What a tool produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    /// Text the model sees.
    pub content: String,
    /// Whether the tool failed.
    ///
    /// A tool that ran and reported a problem is still a result, not an error: the model needs
    /// to read what went wrong in order to do something about it.
    pub is_error: bool,
}

impl Output {
    /// A successful result.
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// A failure the model should read and react to.
    #[must_use]
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
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
