//! The axum wire contract.
//!
//! Every process boundary in axum carries these types and nothing else. This crate performs
//! no I/O and depends on no runtime; `axum-ipc` owns transport, `axum-host` owns meaning.
//!
//! Hard cap: 4,000 lines. Tau's equivalent is 22,750 lines carrying 165 event variants, and
//! that is the direct cause of its 34,875-line daemon. Growth past the cap means capabilities
//! are being added where they should be composed.

mod ids;

pub use ids::{MessageId, SessionId, ToolCallId};

use serde::{Deserialize, Serialize};

/// Protocol version. Stays `0` for as long as axum is the only implementation of each peer.
///
/// Breaking the wire is free while that holds; it becomes expensive the moment third parties
/// depend on it, which is a v0.5 problem.
pub const PROTOCOL_VERSION: u16 = 0;

/// A monotonic position in a session's event log.
///
/// A UI attaches with the last cursor it saw and receives everything after it, so a detached
/// UI can rejoin an in-flight turn without replaying the whole session.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Cursor(pub u64);

impl Cursor {
    /// The position before the first event.
    pub const ZERO: Self = Self(0);

    /// The next position after this one.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Why an assistant turn stopped.
///
/// Re-exported from `axum-model` rather than declared again. The two were identical, and a
/// second copy is the shape of bug where a field is added to one and the other silently keeps
/// answering the old question.
pub use axum_model::StopReason;

/// What the agent is doing, for the status line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AgentStatus {
    /// Waiting for input.
    Idle,
    /// A turn is in flight.
    Working {
        /// Human-readable label, e.g. "Thinking".
        label: String,
    },
    /// A provider call failed and is being retried.
    Retrying {
        /// 1-based attempt number.
        attempt: u32,
        /// Total attempts that will be made.
        max_attempts: u32,
        /// Milliseconds until the next attempt.
        delay_ms: u64,
    },
}

/// How an error should be presented, and whether it is worth retrying.
///
/// An enum built from status codes and typed bodies, never from matching provider prose.
/// Pi classifies with ~35 regex alternates and had to re-prefix Bedrock errors so they matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// The connection failed; retry.
    Transport,
    /// The provider is overloaded; retry with backoff.
    Overload,
    /// Rate limited; retry after the window.
    Throttle,
    /// Credentials are missing or rejected; do not retry.
    Auth,
    /// The request was malformed; do not retry.
    Invalid,
    /// The context window overflowed; compact and retry.
    Overflow,
    /// Unclassified.
    Unknown,
}

impl ErrorClass {
    /// Whether retrying this class of error can succeed.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Transport | Self::Overload | Self::Throttle)
    }
}

/// The outcome of a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Text the model sees.
    pub output: String,
    /// Whether the tool failed.
    pub is_error: bool,
}

/// One rendered entry in a transcript.
///
/// The UI stores a `Vec<Entry>` and renders it; it never reconstructs one from deltas alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Entry {
    /// Something the user submitted.
    User {
        /// Stable identity for this message.
        id: MessageId,
        /// Markdown source.
        text: String,
    },
    /// A model response, possibly still streaming.
    Assistant {
        /// Stable identity for this message.
        id: MessageId,
        /// Markdown source accumulated so far.
        text: String,
        /// Reasoning content, rendered dim and italic.
        thinking: String,
        /// `None` while the turn is in flight.
        stop_reason: Option<StopReason>,
        /// Populated when `stop_reason` is `Error`.
        error: Option<String>,
    },
    /// A tool invocation and its outcome.
    Tool {
        /// Stable identity for this call.
        id: ToolCallId,
        /// Tool name as the model asked for it.
        name: String,
        /// Arguments, pretty-printed JSON.
        args: String,
        /// `None` while the tool is running.
        result: Option<ToolResult>,
    },
}

/// Daemon → UI.
///
/// Nine variants. If this grows past fifteen before M1 ships, capabilities are being added
/// where they should be composed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum HarnessEvent {
    /// Everything the UI needs to draw the transcript from cold, sent on attach.
    SessionSnapshot {
        /// Position of the last entry; subsequent events continue from here.
        cursor: Cursor,
        /// The session being attached to.
        session: SessionId,
        /// Entries in transcript order.
        entries: Vec<Entry>,
        /// What the agent is doing right now.
        status: AgentStatus,
    },
    /// A user message was accepted into the transcript.
    UserMessage {
        /// Position of this event.
        cursor: Cursor,
        /// Stable identity for this message.
        id: MessageId,
        /// Markdown source.
        text: String,
    },
    /// A model response began.
    AssistantStarted {
        /// Position of this event.
        cursor: Cursor,
        /// Stable identity for this message.
        id: MessageId,
    },
    /// Incremental model output.
    AssistantDelta {
        /// Position of this event.
        cursor: Cursor,
        /// The message being extended.
        id: MessageId,
        /// Text appended to the response body.
        text: String,
        /// Text appended to the reasoning block.
        thinking: String,
    },
    /// A model response finished.
    AssistantEnded {
        /// Position of this event.
        cursor: Cursor,
        /// The message that finished.
        id: MessageId,
        /// Why it stopped.
        stop_reason: StopReason,
        /// Populated when `stop_reason` is `Error`.
        error: Option<String>,
    },
    /// A tool call began.
    ToolCallStarted {
        /// Position of this event.
        cursor: Cursor,
        /// Stable identity for this call.
        id: ToolCallId,
        /// Tool name as the model asked for it.
        name: String,
        /// Arguments, pretty-printed JSON.
        args: String,
    },
    /// A tool call finished.
    ToolCallEnded {
        /// Position of this event.
        cursor: Cursor,
        /// The call that finished.
        id: ToolCallId,
        /// What the tool produced.
        result: ToolResult,
    },
    /// The agent changed state.
    StatusChanged {
        /// Position of this event.
        cursor: Cursor,
        /// The new state.
        status: AgentStatus,
    },
    /// Something went wrong outside a turn.
    Error {
        /// Position of this event.
        cursor: Cursor,
        /// How to present it and whether a retry is coming.
        class: ErrorClass,
        /// Human-readable detail.
        message: String,
    },
}

impl HarnessEvent {
    /// The log position this event occupies.
    ///
    /// A snapshot reports the position of its last entry, so attaching with the snapshot's
    /// cursor and replaying from there yields no duplicates.
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        match self {
            Self::SessionSnapshot { cursor, .. }
            | Self::UserMessage { cursor, .. }
            | Self::AssistantStarted { cursor, .. }
            | Self::AssistantDelta { cursor, .. }
            | Self::AssistantEnded { cursor, .. }
            | Self::ToolCallStarted { cursor, .. }
            | Self::ToolCallEnded { cursor, .. }
            | Self::StatusChanged { cursor, .. }
            | Self::Error { cursor, .. } => *cursor,
        }
    }
}

/// UI → daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "command")]
pub enum UiCommand {
    /// Subscribe to a session, replaying everything after `from_cursor`.
    Attach {
        /// The session to attach to, or `None` for the most recent.
        session: Option<SessionId>,
        /// Replay position; [`Cursor::ZERO`] means "from the beginning".
        from_cursor: Cursor,
    },
    /// Submit a prompt.
    SubmitPrompt {
        /// Markdown source.
        text: String,
    },
    /// Interrupt the running turn.
    Interrupt,
    /// Unsubscribe; the turn keeps running.
    Detach,
}

/// A framed message in either direction.
///
/// The version rides every frame so a mismatched peer is rejected at the first read rather
/// than misparsed somewhere deeper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// Always [`PROTOCOL_VERSION`] for frames this build writes.
    pub version: u16,
    /// The message.
    pub body: T,
}

impl<T> Envelope<T> {
    /// Wrap a message for transmission.
    pub const fn new(body: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_advances() {
        assert_eq!(Cursor::ZERO.next(), Cursor(1));
    }

    #[test]
    fn transport_errors_retry_and_auth_errors_do_not() {
        assert!(ErrorClass::Transport.is_retryable());
        assert!(ErrorClass::Overload.is_retryable());
        assert!(!ErrorClass::Auth.is_retryable());
        assert!(!ErrorClass::Invalid.is_retryable());
    }

    #[test]
    fn every_event_reports_its_cursor() {
        let event = HarnessEvent::UserMessage {
            cursor: Cursor(7),
            id: MessageId::new("m1"),
            text: "hi".into(),
        };
        assert_eq!(event.cursor(), Cursor(7));
    }

    #[test]
    fn envelope_stamps_the_current_version() {
        let envelope = Envelope::new(UiCommand::Interrupt);
        assert_eq!(envelope.version, PROTOCOL_VERSION);
    }
}

/// Host → tool peer.
///
/// Five messages, against Tau's sixty-four. What is kept from Tau is the part that matters:
/// a call is journalled before the registry is consulted, so a call that went nowhere is still
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
