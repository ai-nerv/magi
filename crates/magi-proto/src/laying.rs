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

/// One place in a request. `item` and `stub` name entries; the rest are text balthasar wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Slot {
    Pinned {
        #[serde(default)]
        text: String,
    },
    Summary {
        #[serde(default)]
        text: String,
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
    },
    Memory {
        #[serde(default)]
        text: String,
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
}
