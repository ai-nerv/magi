//! The magi wire contract.
//!
//! Every process boundary in magi carries these types and nothing else. This crate performs no
//! I/O and depends on no runtime; `magi-ipc` owns transport, `magi-host` owns meaning. Hard cap:
//! 4,000 lines.
//!
//! Three transports carry it: **argv**, one JSON object on stdout; **pipe**, newline-delimited
//! JSON between a parent and its child; **socket**, four bytes of big-endian length then JSON.
//!
//! ```text
//! -> {"call":"status","args":[]}          a call is answered
//! <- {"ok":true,"family":1,"n":1,"result":[{"busy":false}]}
//!    {"event":"listening","at":"…"}       an event is not
//! ```
//!
//! `result` is always a list and `n` says how long it is; `family` says which revision the reply
//! is written in, and a reader refuses a number it does not know. The tag key is `event`,
//! everywhere, in both directions, and `scripts/gate-wire.sh` refuses any other.

pub mod ask;
mod ids;
pub mod permit;
pub mod setup;
pub mod surfacing;
pub mod tooling;
pub mod wondering;

pub use ids::{MessageId, SessionId, ToolCallId};
pub use tooling::ToolResult;

use serde::{Deserialize, Serialize};

/// Protocol version. Stays `0` for as long as magi is the only implementation of each peer.
pub const PROTOCOL_VERSION: u16 = 0;

/// A monotonic position in a session's event log; a UI attaches with the last one it saw.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Cursor(pub u64);

impl Cursor {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

pub use magi_model::StopReason;

pub use magi_model::Usage;

/// One model a session could switch to, carrying why it cannot be used when it cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub name: String,
    pub context_window: u64,
    pub requirement: String,
    /// Environment variables that would make it ready. Checked by the UI against its own
    /// environment: a daemon outlives the shell that started it and never sees a later export.
    #[serde(default)]
    pub wants_vars: Vec<String>,
    #[serde(default)]
    pub reasoning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub context_window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AgentStatus {
    Idle,
    Working {
        label: String,
    },
    Retrying {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Transport,
    Overload,
    Throttle,
    Auth,
    Invalid,
    Overflow,
    Unknown,
}

impl ErrorClass {
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Transport | Self::Overload | Self::Throttle)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Entry {
    User {
        id: MessageId,
        text: String,
        /// Context the model is given with this prompt. Journalled, and rendered nowhere.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        aside: String,
    },
    Assistant {
        id: MessageId,
        text: String,
        thinking: String,
        stop_reason: Option<StopReason>,
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Signatures::is_empty")]
        signatures: Signatures,
        /// What this turn cost, journalled so a resumed session keeps its totals.
        #[serde(default, skip_serializing_if = "is_free")]
        usage: Usage,
    },
    Tool {
        id: ToolCallId,
        name: String,
        args: String,
        result: Option<ToolResult>,
        /// The third carrier. Google issues one per call rather than per message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    /// Something magi is telling you. Produced by a UI and never journalled.
    Notice { text: String },
    /// Something another magi said to this one, arriving on this session's socket.
    From {
        who: String,
        kin: String,
        /// What sort of message it is: `note`, `question`, `attention`, `trouble`…
        sort: String,
        text: String,
    },
    /// The conversation as it was at an earlier point, taken up again. The skipped entries stay in
    /// the journal and on screen; only the model's view changes.
    Branch {
        id: MessageId,
        /// How many entries from the start remain live; everything up to this record is skipped.
        keeps: usize,
    },
    Compaction {
        id: MessageId,
        summary: String,
        replaces: usize,
    },
    /// One tool result sent as a stub. balthasar marks a turn masked as it hands the plan over, so
    /// a mask applied without being recorded here leaves it planning against a fiction.
    Masked {
        id: MessageId,
        /// Which entry is stubbed, in the space [`Entry::Compaction::replaces`] also counts in.
        at: usize,
        /// What the provider is sent instead, from the tool's own mask handler; a tool with no
        /// handler is never masked.
        shown: String,
    },
}

/// Opaque provider state, handed back verbatim or the provider rejects the request: Anthropic's
/// extended thinking with tools answers a tool-using turn's second round trip with a 400 without
/// it. Never parsed, never generated, never shown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signatures {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl Signatures {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.thinking.is_none()
    }
}

fn is_free(usage: &Usage) -> bool {
    *usage == Usage::default()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum HarnessEvent {
    SessionSnapshot {
        cursor: Cursor,
        session: SessionId,
        entries: Vec<Entry>,
        status: AgentStatus,
        #[serde(default)]
        model: Option<ModelInfo>,
        #[serde(default)]
        choices: Vec<ModelChoice>,
        #[serde(default)]
        thinking: String,
    },
    UserMessage {
        cursor: Cursor,
        id: MessageId,
        text: String,
    },
    MessageArrived {
        cursor: Cursor,
        /// Who said it, as `project/role/id`.
        who: String,
        kin: String,
        sort: String,
        text: String,
    },
    AssistantStarted {
        cursor: Cursor,
        id: MessageId,
    },
    AssistantDelta {
        cursor: Cursor,
        id: MessageId,
        text: String,
        thinking: String,
    },
    AssistantEnded {
        cursor: Cursor,
        id: MessageId,
        stop_reason: StopReason,
        error: Option<String>,
        #[serde(default)]
        usage: Usage,
    },
    ToolCallStarted {
        cursor: Cursor,
        id: ToolCallId,
        name: String,
        args: String,
    },
    ToolCallEnded {
        cursor: Cursor,
        id: ToolCallId,
        result: ToolResult,
    },
    StatusChanged {
        cursor: Cursor,
        status: AgentStatus,
    },
    ModelChanged {
        cursor: Cursor,
        model: Option<ModelInfo>,
    },
    /// A tool is about to do something nothing has allowed yet. The turn stops until
    /// [`UiCommand::Permit`].
    PermissionAsked {
        cursor: Cursor,
        id: ToolCallId,
        tool: String,
        action: crate::permit::Action,
        /// The widths this may be answered at, narrowest first.
        offers: Vec<crate::permit::Scope>,
    },
    /// A tool is asking the person something: the general form of [`Self::PermissionAsked`], with
    /// the tool's own options. The turn stops until [`UiCommand::Answered`].
    Asked {
        cursor: Cursor,
        id: ToolCallId,
        tool: String,
        question: String,
        /// What may be answered, in the order they should be offered.
        options: Vec<crate::tooling::Answer>,
        #[serde(default)]
        detail: Vec<Vec<crate::tooling::Span>>,
    },
    /// A tool has been given rows and will fill them itself; the UI reserves the space and
    /// forwards input without knowing what goes in there.
    Surfaced {
        cursor: Cursor,
        id: ToolCallId,
        tool: String,
        rows: u16,
        /// What it is for, for a UI that cannot draw it.
        about: String,
    },
    Drew {
        id: ToolCallId,
        lines: Vec<Vec<crate::tooling::Span>>,
        /// Where the terminal's cursor belongs, in the surface's coordinates; `None` leaves it in
        /// the prompt. Resolved by the client, the only end that knows where the rows landed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<crate::surfacing::At>,
    },
    Unsurfaced {
        cursor: Cursor,
        id: ToolCallId,
    },
    /// A standing permission was given on a prompt the UI did not draw. The UI otherwise learns
    /// what this session holds only from the answers it sends.
    Granted {
        cursor: Cursor,
        grant: crate::permit::Grant,
    },
    /// Something the UI asked for could not be done. Distinct from [`Self::Error`].
    Refused {
        cursor: Cursor,
        message: String,
    },
    Branched {
        cursor: Cursor,
        id: MessageId,
        keeps: usize,
    },
    Compacted {
        cursor: Cursor,
        id: MessageId,
        summary: String,
        replaces: usize,
    },
    Error {
        cursor: Cursor,
        class: ErrorClass,
        message: String,
    },
}

impl HarnessEvent {
    /// The log position this event occupies. A snapshot reports its last entry's.
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        match self {
            Self::SessionSnapshot { cursor, .. }
            | Self::UserMessage { cursor, .. }
            | Self::MessageArrived { cursor, .. }
            | Self::AssistantStarted { cursor, .. }
            | Self::AssistantDelta { cursor, .. }
            | Self::AssistantEnded { cursor, .. }
            | Self::ToolCallStarted { cursor, .. }
            | Self::ToolCallEnded { cursor, .. }
            | Self::StatusChanged { cursor, .. }
            | Self::Compacted { cursor, .. }
            | Self::PermissionAsked { cursor, .. }
            | Self::Asked { cursor, .. }
            | Self::Surfaced { cursor, .. }
            | Self::Unsurfaced { cursor, .. }
            | Self::Granted { cursor, .. }
            | Self::Refused { cursor, .. }
            | Self::ModelChanged { cursor, .. }
            | Self::Branched { cursor, .. }
            | Self::Error { cursor, .. } => *cursor,
            // A frame occupies no place in the log; nothing replays it.
            Self::Drew { .. } => Cursor::ZERO,
        }
    }
}

/// UI → daemon: the closed list of what a client may ask of a session.
#[path = "commanding.rs"]
mod commanding;
pub use commanding::UiCommand;

/// A framed message in either direction; the version rides every frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// Always [`PROTOCOL_VERSION`] for frames this build writes.
    pub version: u16,
    pub body: T,
}

impl<T> Envelope<T> {
    pub const fn new(body: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            body,
        }
    }
}

#[cfg(test)]
#[path = "encoding.rs"]
mod encoding;

mod peering;
pub use peering::{ToolReport, ToolRequest};
