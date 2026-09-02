//! What an open selection list is choosing, and where its answer goes.
//!
//! Its own file because the answer is the interesting part: the list is a generic widget
//! and every row looks alike, so what separates one list from another is entirely which
//! of these it was opened as. Held beside the list rather than inside it — without it
//! every answer went to the same place, and picking a thinking level asked for a model
//! called "medium".

use magi_proto::ToolCallId;

/// What an open selection list is choosing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picking {
    /// Which model answers.
    Model,
    /// How much reasoning to ask for.
    Thinking,
    /// Which earlier session to continue.
    ///
    /// Carries what each row said beside the id it means, because a row is labelled with what a
    /// person can read — what they asked for — and that is not an id. The picker is taken by the
    /// keypress that chose a row, so by the time this is read there is no list left to index.
    Session {
        /// Every row, as `(what it said, which session it was)`.
        rows: Vec<(String, String)>,
    },
    /// Whether a tool may do what it is about to do.
    ///
    /// Carries the question's id, because the answer has to find its way back to the turn that
    /// is blocked on it, and the widths on offer, because they were computed from the action by
    /// the side that knows what the action was.
    Permission {
        /// Which question is being answered.
        id: ToolCallId,
        /// The widths, in the order they were offered.
        offers: Vec<magi_proto::permit::Scope>,
    },
    /// Whether another session may become this one's child.
    ///
    /// Not a [`Permission`](Self::Permission) even though it looks like one on screen, and the
    /// difference is where the answer goes: a permission unblocks a turn over this session's own
    /// socket, and this goes down the pipe to melchior, which is holding a request another session
    /// is waiting on. Same picker, two entirely different destinations.
    Adoption {
        /// Which request, as melchior named it.
        id: String,
    },
}
