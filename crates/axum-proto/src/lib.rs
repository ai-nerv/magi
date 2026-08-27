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

/// Tokens a turn consumed.
///
/// Re-exported for the same reason [`StopReason`] is: the provider layer already has exactly
/// this, and a second copy is where a field gets added to one and the other keeps answering
/// the old question.
pub use axum_model::Usage;

/// Which model is answering, and how much room it has.
///
/// Sent with the snapshot rather than assumed by the UI. A UI that guessed would be wrong the
/// moment a session was resumed against a different configuration, and "which model am I
/// talking to" is the question a footer exists to answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Qualified name, as `axum models` prints it.
    pub name: String,
    /// Tokens the model accepts, so a UI can show how full the conversation is.
    pub context_window: u64,
}

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
        /// What the provider needs back to continue this reasoning.
        #[serde(default, skip_serializing_if = "Signatures::is_empty")]
        signatures: Signatures,
        /// What this turn cost.
        ///
        /// Journalled rather than counted live, so a resumed session shows the totals it
        /// actually accrued instead of starting again from zero.
        #[serde(default, skip_serializing_if = "is_free")]
        usage: Usage,
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
        /// The third carrier. Google issues one per call rather than per message, which is why
        /// it rides here and not with the message that asked for the call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    /// The conversation as it was at an earlier point, taken up again.
    ///
    /// "That went wrong, back up and try something else." The entries it skips stay in the
    /// journal and stay on screen greyed behind it, because a session is append-only: what a
    /// branch changes is what the model is shown, never what happened.
    Branch {
        /// Stable identity for this record.
        id: MessageId,
        /// How many entries from the start of the session remain live.
        ///
        /// Everything from here up to this record is skipped. A count, like a compaction's,
        /// because both answer the same question: where does the live conversation start.
        keeps: usize,
    },
    /// Everything before this, standing in for itself.
    ///
    /// A conversation outgrows the window long before it stops being useful, and the usual fix
    /// is to drop the beginning — which loses exactly the part that said what the task was.
    /// This replaces it with a summary instead, and only for the provider: the entries it
    /// covers stay in the journal and stay on screen. Sessions are append-only and
    /// delete-never, so compacting adds a record rather than removing any.
    Compaction {
        /// Stable identity for this record.
        id: MessageId,
        /// What stands in for the entries before it.
        summary: String,
        /// How many entries from the start of the session this covers.
        ///
        /// A count rather than a cursor because it is an index into the transcript, and the
        /// transcript is what has to be rebuilt from it.
        replaces: usize,
    },
}

/// Opaque provider state that has to be handed back exactly as it arrived.
///
/// A reasoning model does not send its reasoning back to you in a form you can re-send. It
/// sends a *signature*: a token standing for the thinking it did, which the next request must
/// carry verbatim or the provider rejects it. Anthropic's extended thinking with tools is the
/// case that makes this mandatory rather than nice — the second round trip of a tool-using
/// turn is a 400 without it.
///
/// Never parsed, never generated, never shown. Stored and returned, which is the whole
/// contract, and the reason these are `String` and not a type that invites inspection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signatures {
    /// Carrier for the response body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Carrier for the reasoning block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl Signatures {
    /// Whether there is nothing to carry.
    ///
    /// Used to keep them out of the journal entirely when a provider issues none, so a
    /// transcript from a non-reasoning model reads exactly as it did before they existed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.thinking.is_none()
    }
}

/// Whether a turn cost nothing, so it can be left out of the journal entirely.
///
/// A refusal and a message still streaming both cost nothing yet, and a transcript full of
/// zeroes is harder to read than one that mentions cost only where there was some.
fn is_free(usage: &Usage) -> bool {
    *usage == Usage::default()
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
        /// Which model is answering, when one is configured.
        #[serde(default)]
        model: Option<ModelInfo>,
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
        /// What the turn cost.
        #[serde(default)]
        usage: Usage,
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
    /// The conversation was rewound to an earlier point.
    Branched {
        /// Position of this event.
        cursor: Cursor,
        /// Stable identity for the record.
        id: MessageId,
        /// How many entries from the start remain live.
        keeps: usize,
    },
    /// The conversation was summarised to fit the window.
    Compacted {
        /// Position of this event.
        cursor: Cursor,
        /// Stable identity for the record.
        id: MessageId,
        /// What stands in for the entries before it.
        summary: String,
        /// How many entries from the start of the session it covers.
        replaces: usize,
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
            | Self::Compacted { cursor, .. }
            | Self::Branched { cursor, .. }
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
    /// Rewind the conversation.
    Branch {
        /// How many entries from the start to keep, or `None` for "undo the last exchange".
        ///
        /// Optional because only the daemon knows which entries are still live: after one
        /// rewind the abandoned exchange is still on screen, so a UI counting for itself would
        /// name a message that is already gone.
        keeps: Option<usize>,
    },
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
