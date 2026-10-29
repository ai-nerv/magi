//! UI → daemon: the closed list of what a client can ask a session to do.

use crate::{Cursor, SessionId, ToolCallId, permit};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "command")]
pub enum UiCommand {
    Inspect,
    ReadHistory {
        session: SessionId,
        through: Cursor,
        after: Cursor,
        key: Option<String>,
    },
    AttachLatest {
        #[serde(default)]
        draws: bool,
        /// As [`Self::Attach`]'s: without it the connection only watches.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    Review {
        tool: String,
        action: permit::Action,
        /// The parent's controller key; a review without it is refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    Attach {
        session: Option<SessionId>,
        from_cursor: Cursor,
        /// Whether this client can draw rows a tool asks for; `magi -p` cannot.
        #[serde(default)]
        draws: bool,
        /// The key in `<socket>.key`. Without it the connection watches and may change nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    /// How big the screen is, and what its keyboard can say. Sent at attach and on resize.
    Sized {
        /// Rows a surface could be drawn in once the chrome has its share; `None` where unmeasured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows: Option<u16>,
        cols: u16,
        #[serde(default)]
        holds: bool,
    },
    /// The float's inside, all a surface that asked for the float is given.
    FloatSized {
        rows: u16,
        cols: u16,
    },
    /// A block's inside: the widest a surface drawn in its own call's block is, and the most rows
    /// it may have.
    BlockSized {
        rows: u16,
        cols: u16,
    },
    SubmitPrompt {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<magi_model::Image>,
        text: String,
        /// Context for the model that is not what the person typed; see [`crate::Entry::User::aside`].
        #[serde(default, skip_serializing_if = "String::is_empty")]
        aside: String,
    },
    SubmitTracked {
        request: u64,
        images: Vec<magi_model::Image>,
        text: String,
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
    RefreshModels,
    RefreshWorkflows,
    Environment(Environment),
    SetModel {
        name: String,
    },
    SelectModel {
        request: u64,
        name: String,
    },
    SetThinking {
        level: String,
    },
    SetMode {
        mode: String,
    },
    /// How sure a second model has to be for its verdict to be acted on.
    SetUnsure {
        band: (f64, f64),
    },
    /// Which provider serves the model, by routing tag; `None` lets the router choose.
    SetProvider {
        provider: Option<String>,
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
    /// The person took the screen back: the surface ends and its program with it.
    Unsurface {
        id: ToolCallId,
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
    /// Ask the memory layer about its notes; answered with [`crate::HarnessEvent::MemoryAnswered`].
    Memory {
        verb: String,
        #[serde(default)]
        arg: serde_json::Value,
    },
    Detach,
}

/// A transient environment request, with its contents omitted from diagnostics.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub source: bool,
    pub input: String,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl UiCommand {
    /// The command's wire name, for a log line that must carry nothing a command holds.
    #[must_use]
    pub const fn verb(&self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::ReadHistory { .. } => "read_history",
            Self::AttachLatest { .. } => "attach_latest",
            Self::Review { .. } => "review",
            Self::Environment(_) => "environment",
            Self::Attach { .. } => "attach",
            Self::Sized { .. } => "sized",
            Self::FloatSized { .. } => "float_sized",
            Self::BlockSized { .. } => "block_sized",
            Self::SubmitPrompt { .. } => "submit_prompt",
            Self::SubmitTracked { .. } => "submit_tracked",
            Self::Arrived { .. } => "arrived",
            Self::Interrupt => "interrupt",
            Self::RefreshModels => "refresh_models",
            Self::RefreshWorkflows => "refresh_workflows",
            Self::SetModel { .. } => "set_model",
            Self::SelectModel { .. } => "select_model",
            Self::SetThinking { .. } => "set_thinking",
            Self::SetMode { .. } => "set_mode",
            Self::SetUnsure { .. } => "set_unsure",
            Self::SetProvider { .. } => "set_provider",
            Self::Permit { .. } => "permit",
            Self::Answered { .. } => "answered",
            Self::Keyed { .. } => "keyed",
            Self::Moused { .. } => "moused",
            Self::Unsurface { .. } => "unsurface",
            Self::DeclareNeeds => "declare_needs",
            Self::TakeGrants { .. } => "take_grants",
            Self::Branch { .. } => "branch",
            Self::Resume { .. } => "resume",
            Self::Memory { .. } => "memory",
            Self::Detach => "detach",
        }
    }

    /// Whether a connection that only watches may send this.
    #[must_use]
    pub const fn watches(&self) -> bool {
        matches!(
            self,
            Self::Attach { .. } | Self::AttachLatest { .. } | Self::Inspect | Self::Detach
        )
    }
}

#[cfg(test)]
mod verbs {
    use super::*;

    #[test]
    fn a_verb_is_the_tag_the_command_travels_under() {
        for command in [
            UiCommand::Interrupt,
            UiCommand::DeclareNeeds,
            UiCommand::BlockSized { rows: 8, cols: 40 },
            UiCommand::SetMode { mode: "x".into() },
            UiCommand::SelectModel {
                request: 7,
                name: "fake/one".into(),
            },
            UiCommand::TakeGrants { grants: Vec::new() },
            UiCommand::Inspect,
            UiCommand::AttachLatest {
                draws: false,
                key: None,
            },
            UiCommand::Environment(Environment {
                source: false,
                input: "A=1".into(),
            }),
        ] {
            let wire = serde_json::to_value(&command).expect("encode");
            assert_eq!(wire["command"], command.verb(), "{command:?}");
        }
    }

    #[test]
    fn a_watcher_may_attach_and_leave_and_nothing_else() {
        assert!(UiCommand::Detach.watches());
        assert!(!UiCommand::Interrupt.watches());
        assert!(
            !UiCommand::SelectModel {
                request: 7,
                name: "fake/one".into()
            }
            .watches()
        );
        assert!(
            !UiCommand::SetMode {
                mode: "auto".into()
            }
            .watches()
        );
    }

    #[test]
    fn a_watcher_may_look_but_not_review_or_change_the_environment() {
        assert!(UiCommand::Inspect.watches());
        assert!(
            UiCommand::AttachLatest {
                draws: true,
                key: None
            }
            .watches()
        );
        assert!(
            !UiCommand::Review {
                tool: "read".into(),
                action: permit::Action::Read { path: "/".into() },
                key: None,
            }
            .watches()
        );
        assert!(
            !UiCommand::Environment(Environment {
                source: true,
                input: "PATH=/x".into(),
            })
            .watches()
        );
    }
}
