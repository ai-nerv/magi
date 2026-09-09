//! UI → daemon: the closed list of what a client can ask a session to do.

use crate::{Cursor, SessionId, ToolCallId, permit};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "command")]
pub enum UiCommand {
    Attach {
        session: Option<SessionId>,
        from_cursor: Cursor,
        /// Whether this client can draw rows a tool asks for; `magi -p` cannot.
        #[serde(default)]
        draws: bool,
    },
    /// How big the screen is, and what its keyboard can say. Sent at attach and on resize.
    Sized {
        /// Rows a surface could be drawn in: the room left once the transcript, footer and prompt
        /// have taken theirs, not the window height. `None` where the sender has not measured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
        cols: u16,
        #[serde(default)]
        holds: bool,
    },
    SubmitPrompt {
        text: String,
        /// Context for the model that is not what the person typed; see [`crate::Entry::User::aside`].
        #[serde(default, skip_serializing_if = "String::is_empty")]
        aside: String,
    },
    /// A message arrived from another instance, on the socket the UI binds rather than the daemon's.
    Arrived {
        who: String,
        kin: String,
        sort: String,
        /// What they said. Whether it starts a turn follows from `sort`, in `magi_host::wants_answering`.
        text: String,
    },
    Interrupt,
    SetModel {
        name: String,
    },
    SetThinking {
        level: String,
    },
    Permit {
        id: ToolCallId,
        decision: crate::permit::Decision,
    },
    Answered {
        id: ToolCallId,
        choice: String,
    },
    /// A key the person pressed while a surface held the rows, named — `j`, `enter`, `ctrl+c` — not as bytes.
    Keyed {
        id: ToolCallId,
        key: String,
        #[serde(default)]
        state: crate::surfacing::Held,
    },
    /// The pointer, over rows a surface holds. The coordinates are already the surface's own.
    Moused {
        id: ToolCallId,
        kind: crate::surfacing::Pointed,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<crate::surfacing::Button>,
        row: u16,
        col: u16,
    },
    DeclareNeeds,
    /// Take on the permissions a parent session holds, when it accepts this one as its child.
    /// Additive, and there is no command to take them back.
    TakeGrants {
        grants: Vec<permit::Grant>,
    },
    Branch {
        /// How many entries from the start to keep, or `None` for "undo the last exchange".
        keeps: Option<usize>,
    },
    /// Continue a session recorded earlier, in place of this one; every attached UI follows.
    Resume {
        id: String,
    },
    Detach,
}
