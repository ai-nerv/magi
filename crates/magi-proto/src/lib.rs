//! The magi wire contract.
//!
//! Every process boundary in magi carries these types and nothing else. This crate performs
//! no I/O and depends on no runtime; `magi-ipc` owns transport, `magi-host` owns meaning.
//!
//! Hard cap: 4,000 lines. Tau's equivalent is 22,750 lines carrying 165 event variants, and
//! that is the direct cause of its 34,875-line daemon. Growth past the cap means capabilities
//! are being added where they should be composed.
//!
//! # How this family talks
//!
//! Three transports, two shapes, one encoding. Written out here because it was written out
//! nowhere: four wires had grown four different ways to say the same thing — `say`/`heard`,
//! `to`/`from`, `message`, and a call envelope — and nothing anywhere said which was meant.
//!
//! **Three transports, and the choice between them is about what is being asked.**
//!
//! | | |
//! |---|---|
//! | **argv** | a question with an answer and nothing to hold open. One JSON object on stdout. |
//! | **pipe** | a parent and the child it started. Newline-delimited JSON, both directions. |
//! | **socket** | anything may knock. Four bytes of big-endian length, then JSON. |
//!
//! JSON is on all three. It is the *encoding*, not a transport, and naming it as one is how the
//! diagram of this family came to have "argv + json" on an edge.
//!
//! **Two shapes, and the difference is whether anybody is waiting.**
//!
//! A **call** is answered:
//!
//! ```text
//! -> {"call":"status","args":[]}
//! <- {"ok":true,"family":1,"n":1,"result":[{"busy":false}]}
//! ```
//!
//! An **event** is not:
//!
//! ```text
//! {"event":"listening","at":"…"}
//! ```
//!
//! `result` is a **list** and `n` says how long it is: a sibling that unpacks a list would read
//! a bare value as *nothing at all*, so an answer would come back empty rather than wrong — and
//! an empty answer looks like an empty session. `family` says which revision of this the reply
//! is written in; a reader refuses a number it does not know and tolerates one it predates.
//!
//! A refused call is a **reply**, not a dropped connection. The caller then sees the far end's
//! error rather than a transport error, and "no such call: nope" says what to fix where
//! "connection reset" does not.
//!
//! **The tag key is `event`, everywhere, in both directions.** `scripts/gate-wire.sh` refuses
//! any other, because the failure mode is silent: casper is another checkout with its own copy
//! of these frames, so when two spellings drift nothing fails — the surface simply stops being
//! answered.

pub mod ask;
mod ids;
pub mod permit;
pub mod setup;
pub mod surfacing;
pub mod tooling;
pub mod wondering;

pub use ids::{MessageId, SessionId, ToolCallId};
// Re-exported: it is what a tool produced, so it lives beside the rest of that contract, and
// every reader of a transcript already reaches for it here.
pub use tooling::ToolResult;

use serde::{Deserialize, Serialize};

/// Protocol version. Stays `0` for as long as magi is the only implementation of each peer.
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
/// Re-exported from `magi-model` rather than declared again. The two were identical, and a
/// second copy is the shape of bug where a field is added to one and the other silently keeps
/// answering the old question.
pub use magi_model::StopReason;

/// Tokens a turn consumed.
///
/// Re-exported for the same reason [`StopReason`] is: the provider layer already has exactly
/// this, and a second copy is where a field gets added to one and the other keeps answering
/// the old question.
pub use magi_model::Usage;

/// One model a session could switch to.
///
/// Carries why it cannot be used rather than being left out when it cannot. A list filtered to
/// what already works is empty for somebody who has set no keys, and an empty list teaches
/// nothing: the question they are asking is precisely "what could I use, and what would it
/// take" — which is the moment they most need an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    /// Qualified name, as `magi models` prints it.
    pub name: String,
    /// Tokens it accepts.
    pub context_window: u64,
    /// Empty when it is ready; otherwise what to do about it.
    pub requirement: String,
    /// Environment variables that would make it ready, if that is what it needs.
    ///
    /// Carried structurally rather than parsed back out of `requirement`, because the UI has to
    /// answer a question the daemon cannot: whether *this* process can see a variable the
    /// daemon could not. A daemon outlives the shell that started it, so a key exported
    /// afterwards never reaches it, and "set OPENROUTER_API_KEY" is then a lie told to somebody
    /// who has already set it.
    #[serde(default)]
    pub wants_vars: Vec<String>,
    /// Whether it can reason, so a thinking level can be offered or refused.
    #[serde(default)]
    pub reasoning: bool,
}

/// Which model is answering, and how much room it has.
///
/// Sent with the snapshot rather than assumed by the UI. A UI that guessed would be wrong the
/// moment a session was resumed against a different configuration, and "which model am I
/// talking to" is the question a footer exists to answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Qualified name, as `magi models` prints it.
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
        /// Context the model is given with this prompt and nobody is shown.
        ///
        /// Naming `$iota-mu` puts facts about that instance in front of the model: who it is,
        /// how it stands to this session, what has already passed between them. The person
        /// typed one line and should see one line — pasting the briefing into the transcript
        /// showed them a wall of text they did not write and could not have wanted.
        ///
        /// Journalled, because the model has to still see it on the next turn, and rendered
        /// nowhere.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        aside: String,
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
    /// Something magi is telling you, rather than something the model said.
    ///
    /// `/help`, a refused `/model`, an unknown command. Its own kind because a notice rendered
    /// as an assistant message *is* an assistant message as far as anyone reading is
    /// concerned, and "the model just printed a keybinding reference" is a confusing thing to
    /// believe.
    ///
    /// Produced by a UI and never journalled: the daemon has no reason to make one, and a
    /// transcript replayed from disk should hold the conversation rather than one UI's
    /// running commentary on it.
    Notice {
        /// Markdown, rendered like any other prose.
        text: String,
    },
    /// Something another magi said to this one.
    ///
    /// Its own kind rather than a [`Entry::User`] with a note on it. The two look alike on
    /// purpose — both are somebody addressing this session, and both are answered the same way
    /// — but the person did not say this, and a block that reads as something you typed is one
    /// you will answer as though you had.
    ///
    /// Produced by a UI, like [`Entry::Notice`]: the message arrived on this session's socket,
    /// which is a UI's, and the daemon on the far end of the other socket never saw it.
    From {
        /// Who said it, as `project/id`.
        who: String,
        /// How they stand to this session: `parent`, `child`, `sibling`, `main`, `cousin`.
        ///
        /// Carried rather than looked up when it is drawn, because it was true when the
        /// message arrived and a session that has since forked would redraw the transcript
        /// with a relation that did not hold at the time.
        kin: String,
        /// What sort of message it is: `note`, `question`, `attention`, `trouble`…
        sort: String,
        /// Markdown, rendered like any other prose.
        text: String,
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
        /// Everything this session could switch to.
        #[serde(default)]
        choices: Vec<ModelChoice>,
        /// How much reasoning is being asked for.
        #[serde(default)]
        thinking: String,
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
    /// A message from another instance was accepted into the transcript.
    ///
    /// The aside on a [`Entry::User`] has no event of its own because nothing on the UI's end
    /// reads it. This does: it is drawn.
    MessageArrived {
        /// Position of this event.
        cursor: Cursor,
        /// Who said it, as `project/role/id`.
        who: String,
        /// How they stood to this session when it arrived.
        kin: String,
        /// What sort of message it is.
        sort: String,
        /// What they said.
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
    /// The session now answers with a different model.
    ///
    /// Its own event rather than a re-sent snapshot: a snapshot carries the transcript, and a
    /// UI folding one has to decide what it has already written to scrollback. Which model is
    /// answering is one fact, and one fact is what this carries.
    ModelChanged {
        /// Position of this event.
        cursor: Cursor,
        /// The model now answering, or `None` if there is none.
        model: Option<ModelInfo>,
    },
    /// A tool is about to do something nothing has allowed yet.
    ///
    /// The turn stops here. Nothing else can decide this: the daemon knows what is about to
    /// happen and the person knows whether they want it, and only one of them is at the
    /// keyboard. Answered with [`UiCommand::Permit`].
    PermissionAsked {
        /// Position of this event.
        cursor: Cursor,
        /// Which question this is, so the answer can be matched to it.
        id: ToolCallId,
        /// The tool that wants to act.
        tool: String,
        /// What it is about to do.
        action: crate::permit::Action,
        /// The widths this may be answered at, narrowest first.
        offers: Vec<crate::permit::Scope>,
    },
    /// A tool is asking the person something, and waiting.
    ///
    /// The general form of [`Self::PermissionAsked`]: any tool may stop and put a question, with
    /// its own options rather than permission's scopes. That is what makes a selection tool, a
    /// confirmation and a form one mechanism instead of three. Answered with
    /// [`UiCommand::Answered`].
    ///
    /// The turn stops here, exactly as it does for a permission: a tool that asked and carried
    /// on would be asking for a record rather than a decision.
    Asked {
        /// Position of this event.
        cursor: Cursor,
        /// Which question this is, so the answer can be matched to it.
        id: ToolCallId,
        /// The tool that is asking.
        tool: String,
        /// What is being asked, in one line.
        question: String,
        /// What may be answered, in the order they should be offered.
        options: Vec<crate::tooling::Answer>,
        /// More about what is being asked, painted, for the rows under the question.
        #[serde(default)]
        detail: Vec<Vec<crate::tooling::Span>>,
    },
    /// A tool has been given rows, and will fill them itself.
    ///
    /// The UI reserves the space and forwards what the person does to it. It does not know or ask
    /// what goes in there — a permission prompt, a file picker and a game are the same event.
    Surfaced {
        /// Position of this event.
        cursor: Cursor,
        /// Which surface this is, so keys and frames can be matched to it.
        id: ToolCallId,
        /// The tool holding the rows.
        tool: String,
        /// How many rows it was given.
        rows: u16,
        /// What it is for, for a UI that cannot draw it.
        about: String,
    },
    /// What a surface drew, this frame.
    ///
    /// Sent as often as the surface redraws, which for something animating is many times a
    /// second. Transient by construction: it is not in the transcript, because what a game looked
    /// like three frames ago is not part of the conversation.
    Drew {
        /// Which surface drew it.
        id: ToolCallId,
        /// The rows, in the same roles everything else is painted in.
        lines: Vec<Vec<crate::tooling::Span>>,
        /// Where the terminal's own cursor belongs, in the surface's coordinates.
        ///
        /// `None` leaves it in the prompt. Carried through rather than resolved here, because the
        /// session has no screen: only the client that drew the rows knows where they landed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<crate::surfacing::At>,
    },
    /// A surface has finished and its rows are given back.
    Unsurfaced {
        /// Position of this event.
        cursor: Cursor,
        /// Which surface ended.
        id: ToolCallId,
    },
    /// A standing permission was given, on a prompt the UI did not draw.
    ///
    /// The UI remembers what this session holds so it can lend it to a child, and it learns that
    /// from the answers it *sends*. A permission answered on a surface is decided on the tool
    /// thread and never passes through it — so it is told, or a child would inherit everything
    /// answered at a picker and nothing answered at a surface.
    Granted {
        /// Position of this event.
        cursor: Cursor,
        /// What was allowed.
        grant: crate::permit::Grant,
    },
    /// Something the UI asked for could not be done, with the reason.
    ///
    /// Distinct from [`Self::Error`], which is the session going wrong. This is a request that
    /// was understood and declined — a model that is not configured, a name that matches
    /// nothing — and the answer belongs on screen rather than in the transcript.
    Refused {
        /// Position of this event.
        cursor: Cursor,
        /// What could not be done, and why.
        message: String,
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
            // A frame is not a position. What a surface drew three frames ago is not part of the
            // conversation and nothing replays it, so it occupies no place in the log.
            Self::Drew { .. } => Cursor::ZERO,
        }
    }
}

/// UI → daemon: the closed list of what a client may ask of a session.
#[path = "commanding.rs"]
mod commanding;
pub use commanding::UiCommand;

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

/// What the protocol's own types encode to.
#[cfg(test)]
#[path = "encoding.rs"]
mod encoding;

mod peering;
pub use peering::{ToolReport, ToolRequest};
