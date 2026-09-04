//! UI → daemon: everything a person's client can ask a session to do.
//!
//! The other half of [`crate::HarnessEvent`], and deliberately the smaller one. A session
//! *reports* whatever happens to it; the things that can be asked of it are a closed list, and
//! keeping that list here is what makes it readable as one.

use crate::{Cursor, SessionId, ToolCallId, permit};
use serde::{Deserialize, Serialize};

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
        /// Whether this client can draw rows a tool asks for, and send it keys.
        ///
        /// `magi -p` cannot: it has no terminal and nobody is watching it. A session that
        /// reserved rows for it would hold the turn open until the surface timed out, waiting on
        /// a keypress that was never coming.
        ///
        /// Defaulted to false so a client that predates surfaces is treated as one that cannot
        /// draw them, which is exactly what it is.
        #[serde(default)]
        draws: bool,
    },
    /// How wide the screen is, and what its keyboard can say.
    ///
    /// Sent at attach and again when the window changes. The session cannot know either — it has
    /// no terminal — and a tenant told a made-up width draws for a screen that is not there.
    Sized {
        /// Columns.
        cols: u16,
        /// Whether this terminal reports key repeats and releases.
        ///
        /// The Kitty keyboard protocol. Without it every key arrives as a bare press, so nothing
        /// drawing in a surface can tell a tap from a hold — and one waiting for a release that is
        /// never coming would look broken on precisely the terminals that cannot send one.
        #[serde(default)]
        holds: bool,
    },
    /// Submit a prompt.
    SubmitPrompt {
        /// Markdown source.
        text: String,
        /// Context for the model that is not part of what the person typed.
        ///
        /// See [`Entry::User::aside`]. Separate on the wire so the session can journal the two
        /// apart: one is the conversation, the other is what the harness knew at the time.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        aside: String,
    },
    /// A message arrived from another instance.
    ///
    /// The UI binds the socket other instances reach this one at, so a message lands there and
    /// nowhere else — and the session, which owns the transcript and runs the turns, is on the
    /// other side of this command. Without it an arriving message reached the screen and
    /// stopped: the model never saw it, so nothing ever answered one.
    Arrived {
        /// Who said it, as `project/role/id`.
        who: String,
        /// How they stand to this session, in one word.
        kin: String,
        /// What sort of message it is.
        sort: String,
        /// What they said.
        ///
        /// Whether it starts a turn is not on the wire: it follows from `sort`, and the one
        /// place that decides is `magi_host::wants_answering`. Carrying the answer as well
        /// would be two rules for one question, in two vocabularies, free to drift.
        text: String,
    },
    /// Interrupt the running turn.
    Interrupt,
    /// Answer with a different model from here on.
    ///
    /// The daemon's choice to make: it holds the catalog this session started with, and only
    /// it knows whether the name resolves to something reachable.
    SetModel {
        /// Qualified or bare name, as `magi models` prints it.
        name: String,
    },
    /// Ask for more or less reasoning from here on.
    SetThinking {
        /// A level, as `magi.thinking` names them.
        level: String,
    },
    /// Answer a [`HarnessEvent::PermissionAsked`].
    ///
    /// An answer that names an unknown id is dropped: the turn it belonged to is over, and
    /// acting on it would allow something nobody is waiting for.
    Permit {
        /// Which question is being answered.
        id: ToolCallId,
        /// What was decided.
        decision: crate::permit::Decision,
    },
    /// Answer a [`HarnessEvent::Asked`].
    ///
    /// An answer that names an unknown id is dropped, for the same reason a permission's is: the
    /// turn it belonged to is over, and acting on it would resume something nobody is waiting on.
    Answered {
        /// Which question is being answered.
        id: ToolCallId,
        /// The id of the option that was chosen.
        choice: String,
    },
    /// A key the person pressed while a surface held the rows.
    ///
    /// By name — `j`, `enter`, `esc`, `ctrl+c` — not as the bytes the terminal sent. The UI has
    /// already decoded one to get here, and handing on the encoding would make every tenant learn
    /// this terminal's.
    Keyed {
        /// Which surface it was meant for.
        id: ToolCallId,
        /// The key, named.
        key: String,
        /// Whether it went down, repeated, or came back up.
        #[serde(default)]
        state: crate::tooling::Held,
    },
    /// The pointer, over rows a surface holds.
    ///
    /// **Already translated.** The UI knows where it drew the reservation; the session does not
    /// and never will. So the coordinates that cross here are the surface's own, and a click that
    /// landed anywhere else was never sent.
    Moused {
        /// Which surface it was meant for.
        id: ToolCallId,
        /// What the pointer did.
        kind: crate::tooling::Pointed,
        /// Which button, for the things a button does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<crate::tooling::Button>,
        /// Rows down from the surface's own first row.
        row: u16,
        /// Columns across from the surface's own first column.
        col: u16,
    },
    /// Ask the model what the work ahead will need, and offer those permissions.
    ///
    /// A proposal, not a decision: every need it names goes through the same prompt any other
    /// request would, so the model gains no authority it did not have. What it buys is being
    /// asked once, about an accurate description of the job, instead of four times in the
    /// middle of it.
    DeclareNeeds,
    /// Take on the permissions a parent session holds.
    ///
    /// Sent once, when somebody at another session accepts this one as its child. The grants are
    /// that session's own — written in its config, or answered into its ledger by a person — so
    /// nothing arrives here that was not consented to once already.
    ///
    /// Additive, and there is no command to take them back: a session that should not have them
    /// is a session that should be restarted. Undoing a grant mid-run would leave a turn holding
    /// a permission it had checked and no longer has.
    TakeGrants {
        /// What the parent may do, and now this session may too.
        grants: Vec<permit::Grant>,
    },
    /// Rewind the conversation.
    Branch {
        /// How many entries from the start to keep, or `None` for "undo the last exchange".
        ///
        /// Optional because only the daemon knows which entries are still live: after one
        /// rewind the abandoned exchange is still on screen, so a UI counting for itself would
        /// name a message that is already gone.
        keeps: Option<usize>,
    },
    /// Continue a session recorded earlier, in place of this one.
    ///
    /// Named by id, which is a journal's file stem. The daemon swaps journals and publishes a
    /// fresh snapshot, so every attached UI follows rather than only the one that asked.
    Resume {
        /// Which session, as [`SessionId`] names it.
        id: String,
    },
    /// Unsubscribe; the turn keeps running.
    Detach,
}
