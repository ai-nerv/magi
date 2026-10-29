//! Magi's process-boundary types, without I/O or runtime dependencies.
//! `magi-ipc` transports them; `magi-host` implements their operations.
//! Family JSON replies carry a revision, result list and count; events carry an `event` tag.
//! Local UI commands and host events use versioned, length-prefixed CBOR envelopes.

pub mod ask;
pub mod caching;
mod ids;
pub mod judging;
pub mod laying;
pub mod monitoring;
pub mod permit;
pub mod setup;
pub mod surfacing;
pub mod tooling;
pub mod wondering;
pub mod workflow;

pub use ids::{MessageId, SessionId, ToolCallId};
pub use tooling::ToolResult;

use serde::{Deserialize, Serialize};

/// Magi-local framed IPC revision, independent of family and persisted-history formats.
pub const PROTOCOL_VERSION: u16 = 3;

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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub vision: bool,
    pub name: String,
    pub context_window: u64,
    pub requirement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ask::Pricing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popularity_rank: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<magi_model::Capabilities>,
    /// Environment variables required to make this model ready.
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

/// Agent roster state: idle means ready for work; finished means assigned work is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    #[default]
    Idle,
    Working,
    /// Cannot go on until something else happens; `waiting`/`blocked` carry a cause.
    Waiting,
    Blocked,
    Finished,
    Gone,
}

impl Phase {
    /// The wire word: the lowercase name the roster carries and every harness parses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Finished => "finished",
            Self::Gone => "gone",
        }
    }

    /// Parse a wire word, returning `None` for an unknown phase.
    #[must_use]
    pub fn read(word: &str) -> Option<Self> {
        Some(match word {
            "starting" => Self::Starting,
            "idle" => Self::Idle,
            "working" => Self::Working,
            "waiting" => Self::Waiting,
            "blocked" => Self::Blocked,
            "finished" => Self::Finished,
            "gone" => Self::Gone,
            _ => return None,
        })
    }
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
    Workflow {
        id: MessageId,
        text: String,
        aside: String,
        images: Vec<magi_model::Image>,
        workflow: Box<workflow::Workflow>,
    },
    User {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<magi_model::Image>,
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
    /// Rewind the model's context while retaining skipped entries in history and display.
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
    /// A tool result replaced by a stub in the provider's context.
    Masked {
        id: MessageId,
        /// Which entry is stubbed, in the space [`Entry::Compaction::replaces`] also counts in.
        at: usize,
        /// Replacement content returned by the tool's mask handler.
        shown: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryPage {
    pub session: SessionId,
    pub transcript: Option<String>,
    pub through: Cursor,
    pub after: Cursor,
    pub next: Option<Cursor>,
    pub entries: Vec<(Cursor, Entry)>,
}

/// Opaque provider state returned verbatim, never parsed, generated or displayed.
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
    TurnStarted {
        owner: u64,
        request: Option<u64>,
    },
    TurnFinished {
        owner: u64,
    },
    PromptRefused {
        request: u64,
        message: String,
    },
    ModelSelected {
        request: u64,
        error: Option<String>,
    },
    WorkflowMessage {
        cursor: Cursor,
        id: MessageId,
        text: String,
        images: Vec<magi_model::Image>,
        workflow: Box<workflow::Workflow>,
    },
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
        #[serde(default)]
        judging: crate::judging::Judging,
    },
    UserMessage {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<magi_model::Image>,
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
    /// Who is asked about what no rule covers, and on what terms, as it stands now.
    ModeChanged {
        cursor: Cursor,
        judging: crate::judging::Judging,
    },
    ModelChanged {
        cursor: Cursor,
        model: Option<ModelInfo>,
    },
    /// A tool action awaiting authorization through [`UiCommand::Permit`].
    PermissionAsked {
        cursor: Cursor,
        id: ToolCallId,
        tool: String,
        action: crate::permit::Action,
        /// The widths this may be answered at, narrowest first.
        offers: Vec<crate::permit::Scope>,
        /// What a second model made of it, when one is configured to say.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        advice: Option<crate::judging::Advice>,
    },
    /// A tool's question awaiting a choice through [`UiCommand::Answered`].
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
    /// A tool-controlled surface whose reserved rows receive forwarded UI input.
    Surfaced {
        cursor: Cursor,
        id: ToolCallId,
        tool: String,
        rows: u16,
        /// What it is for, for a UI that cannot draw it.
        about: String,
        /// Rows in the prompt box, the whole float, or its own call's block.
        #[serde(default)]
        place: crate::tooling::Place,
        /// The tool call that asked for the rows, whose block they are drawn in.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call: Option<ToolCallId>,
        /// Whether the call answered at once and the rows stay open until the person closes them.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        detach: bool,
    },
    Drew {
        id: ToolCallId,
        lines: Vec<Vec<crate::tooling::Span>>,
        /// Cursor position in surface coordinates, or `None` to leave it in the prompt.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<crate::surfacing::At>,
    },
    Unsurfaced {
        cursor: Cursor,
        id: ToolCallId,
    },
    /// A standing session permission granted outside this UI.
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
    /// Transient memory-layout diagnostics for the model card and context view.
    ContextLaid {
        id: String,
        #[serde(default)]
        budget: serde_json::Value,
        #[serde(default)]
        counts: Laid,
        #[serde(default)]
        why: String,
        /// Every slot, in the order it was sent.
        #[serde(default)]
        slots: Vec<laying::LaidSlot>,
    },
    /// Transient request accounting and admission outcome.
    RequestAdmitted {
        /// The whole window, what one request may occupy of it, and what is held back.
        capacity: u64,
        limit: u64,
        reply: u64,
        margin: u64,
        /// The complete input: instructions, declarations and every message.
        counted: u64,
        /// `estimated` or `verified`.
        counting: String,
        /// `admitted`, `replanned`, `refused` or `blocked`, and why when it was not admitted.
        outcome: String,
        #[serde(default)]
        why: String,
    },
    /// Provider-neutral frame changes before an ask; neither cache hits nor durable history.
    CacheFrame {
        reasons: Vec<caching::Invalidation>,
    },
    /// Provider serialization observations; no policy fingerprints or request contents.
    ProviderFrame {
        reasons: Vec<caching::Invalidation>,
    },
    /// What one helper-model job cost, for the cost view. Not part of the log.
    HelperSpent {
        role: String,
        model: String,
        usage: Usage,
    },
    /// A user-facing notice outside conversation history and model context.
    Noticed {
        cursor: Cursor,
        text: String,
    },
    /// The current model choices after an explicit refresh. Not part of the log.
    ModelsRefreshed {
        choices: Vec<ModelChoice>,
        #[serde(default)]
        warning: Option<String>,
    },
    WorkflowsListed {
        entries: Vec<workflow::Summary>,
        warnings: Vec<String>,
        error: Option<String>,
    },
    /// What the memory layer answered a [`UiCommand::Memory`] with. Not part of the log.
    MemoryAnswered {
        verb: String,
        answer: serde_json::Value,
    },
}

/// How many of each kind of slot a layout held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Laid {
    pub items: usize,
    pub stubs: usize,
    pub dropped: usize,
    pub summary: usize,
    pub memory: usize,
    pub pinned: usize,
    pub notes: usize,
}

impl HarnessEvent {
    /// The log position this event occupies. A snapshot reports its last entry's.
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        match self {
            Self::SessionSnapshot { cursor, .. }
            | Self::UserMessage { cursor, .. }
            | Self::WorkflowMessage { cursor, .. }
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
            | Self::ModeChanged { cursor, .. }
            | Self::Branched { cursor, .. }
            | Self::Noticed { cursor, .. }
            | Self::Error { cursor, .. } => *cursor,
            // A frame occupies no place in the log; nothing replays it.
            Self::Drew { .. }
            | Self::TurnStarted { .. }
            | Self::TurnFinished { .. }
            | Self::PromptRefused { .. }
            | Self::ModelSelected { .. }
            | Self::ContextLaid { .. }
            | Self::RequestAdmitted { .. }
            | Self::CacheFrame { .. }
            | Self::ProviderFrame { .. }
            | Self::HelperSpent { .. }
            | Self::ModelsRefreshed { .. }
            | Self::WorkflowsListed { .. }
            | Self::MemoryAnswered { .. } => Cursor::ZERO,
        }
    }
}

/// UI → daemon: the closed list of what a client may ask of a session.
#[path = "commanding.rs"]
mod commanding;
pub use commanding::{Environment, UiCommand};

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

#[cfg(test)]
mod phase_tests {
    use super::Phase;

    #[test]
    fn every_phase_round_trips_through_its_wire_word() {
        for phase in [
            Phase::Starting,
            Phase::Idle,
            Phase::Working,
            Phase::Waiting,
            Phase::Blocked,
            Phase::Finished,
            Phase::Gone,
        ] {
            assert_eq!(
                Phase::read(phase.as_str()),
                Some(phase),
                "{}",
                phase.as_str()
            );
        }
        assert_eq!(
            Phase::read("something-newer"),
            None,
            "an unknown phase is not guessed"
        );
    }
}
