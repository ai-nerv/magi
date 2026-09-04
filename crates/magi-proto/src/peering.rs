//! What the host says to a tool peer, and what it says back.
//!
//! Split out under THE RULE; the wire contract next door is what these belong to.
//!
//! Five messages, against Tau's sixty-four. What is kept from Tau is the part that matters: a
//! call is journalled before the registry is consulted, so a call that went nowhere is still
//! auditable, and a finished call cannot be resurrected by a repeated id.

use crate::ToolCallId;
use serde::{Deserialize, Serialize};

/// auditable, and a finished call cannot be resurrected by a repeated id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "message")]
pub enum ToolRequest {
    /// Run this.
    Call {
        /// Identity the peer must quote back.
        id: ToolCallId,
        /// Which tool, since one peer may offer several.
        name: String,
        /// Arguments, as the model produced them.
        arguments: serde_json::Value,
    },
    /// Stop the call, because the user interrupted or the turn was abandoned.
    ///
    /// Cancellation is in the first cut rather than added later: `esc` has to kill a running
    /// shell, and that cannot be retrofitted onto a bare request/response pair.
    Cancel {
        /// The call to stop.
        id: ToolCallId,
    },
}

/// Tool peer → host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "message")]
pub enum ToolReport {
    /// What this peer offers, sent once on connect.
    ///
    /// Declared by the peer rather than configured by the host: the peer is the only thing
    /// that knows what it can actually do, and a host that guessed would drift.
    Declare {
        /// Tool name, which is also its identity in the registry.
        name: String,
        /// What it does, in the model's terms.
        description: String,
        /// JSON Schema for its arguments.
        parameters: serde_json::Value,
    },
    /// Output so far, for a tool that takes long enough to be worth watching.
    Progress {
        /// The call this belongs to.
        id: ToolCallId,
        /// Text appended to what the call has produced.
        chunk: String,
    },
    /// The call finished.
    Result {
        /// The call that finished.
        id: ToolCallId,
        /// What it produced.
        output: String,
        /// Whether it failed. A tool that ran and reported a problem is still a result.
        is_error: bool,
    },
}
