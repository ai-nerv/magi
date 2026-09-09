//! What an open selection list is choosing, and where its answer goes.

use magi_proto::ToolCallId;

/// What an open selection list is choosing. Rows carry `(label, meaning)` because the picker is
/// taken by the keypress that chose a row, leaving no list to index by position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picking {
    Model,
    Thinking,
    Session {
        rows: Vec<(String, String)>,
    },
    Asked {
        id: ToolCallId,
        rows: Vec<(String, String)>,
    },
    Permission {
        id: ToolCallId,
        offers: Vec<magi_proto::permit::Scope>,
    },
    /// Answered down the pipe to melchior, not over this session's own socket.
    Adoption {
        id: String,
    },
}

impl Picking {
    /// Whether a caller — a turn on this socket, or a request held in melchior — waits on this.
    #[must_use]
    pub const fn blocking(&self) -> bool {
        matches!(
            self,
            Self::Permission { .. } | Self::Asked { .. } | Self::Adoption { .. }
        )
    }
}
