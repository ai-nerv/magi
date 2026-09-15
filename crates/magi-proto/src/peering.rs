//! What the host says to a tool peer, and what it says back. A finished call cannot be
//! resurrected by a repeated id.

use crate::ToolCallId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum ToolRequest {
    Call {
        id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
    },
    /// Stop the call, because the user interrupted or the turn was abandoned.
    Cancel { id: ToolCallId },
}

/// Tool peer → host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum ToolReport {
    /// What this peer offers, sent once on connect.
    Declare {
        name: String,
        description: String,
        parameters: serde_json::Value,
    },
    Progress {
        id: ToolCallId,
        chunk: String,
    },
    Result {
        id: ToolCallId,
        output: String,
        /// Whether it failed. A tool that ran and reported a problem is still a result.
        is_error: bool,
    },
}
