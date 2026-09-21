//! What balthasar answers when asked how to lay out a request, and the helper work it hands magi to
//! run. The memory role's wire shapes, read by the host and kept by the session between prompts.

use serde::{Deserialize, Serialize};

/// What balthasar said to send, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// What to quote back in `applied` and `overflowed`. Empty for a layout magi made up itself.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub budget: serde_json::Value,
    #[serde(default)]
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default = "fits")]
    pub fits: bool,
    #[serde(default)]
    pub why: String,
}

const fn fits() -> bool {
    true
}

/// One place in a request. `item` and `stub` name entries; the rest are text balthasar wrote, with
/// what balthasar reckons it costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Slot {
    Rules {
        #[serde(default)]
        text: String,
        #[serde(default)]
        tokens: u64,
    },
    Observations {
        #[serde(default)]
        text: String,
        #[serde(default)]
        tokens: u64,
    },
    /// Legacy mixed rule/observation text, without a verified authority boundary.
    Pinned {
        #[serde(default)]
        text: String,
        #[serde(default)]
        tokens: u64,
    },
    Summary {
        #[serde(default)]
        text: String,
        #[serde(default)]
        tokens: u64,
    },
    Item {
        cursor: u64,
    },
    Stub {
        cursor: u64,
        #[serde(default)]
        text: String,
    },
    Note {
        #[serde(default)]
        text: String,
        #[serde(default)]
        tokens: u64,
    },
    Memory {
        #[serde(default)]
        text: String,
        #[serde(default)]
        tokens: u64,
        #[serde(default)]
        injection: Option<String>,
    },
    /// A kind this build does not know, skipped rather than failing the whole layout.
    #[serde(other)]
    Other,
}

impl Slot {
    /// The entry this slot sends, when it sends one.
    #[must_use]
    pub const fn cursor(&self) -> Option<u64> {
        match self {
            Self::Item { cursor } | Self::Stub { cursor, .. } => Some(*cursor),
            _ => None,
        }
    }
}

/// One slot as a screen shows it: which kind, which entry, what it costs, and a line of what it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaidSlot {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<u64>,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub text: String,
}

/// One piece of helper work. Generic: magi never reads what it is for, only how to run it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub role: String,
    /// `main` to run it with the session's own model when no helper is configured; else skipped.
    #[serde(default)]
    pub fallback: String,
    #[serde(default)]
    pub instruction: String,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub blocking: bool,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Whether the schema may go to the provider as a schema rather than in words, for a model
    /// that reads nothing else. Off by default: a model that writes is told it in words, since
    /// one held to a schema from the first token has nowhere to think but inside the strings.
    #[serde(default)]
    pub structured: bool,
    /// How much this job needs the model to reason, when it needs any.
    ///
    /// Absent means none, which is what a helper wants: quick and cheap, and one left to reason
    /// can spend a whole budget thinking and answer nothing. A job that has to work something
    /// out rather than answer a narrow question says so — extraction reads a whole transcript
    /// and returns nothing at all without it.
    #[serde(default)]
    pub thinking: Option<String>,
}
