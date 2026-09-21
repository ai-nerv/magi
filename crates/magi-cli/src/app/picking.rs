//! What an open selection list is choosing, and where its answer goes.

use magi_proto::{ToolCallId, UiCommand};

/// What an open selection list is choosing. Rows carry `(label, meaning)` because the picker is
/// taken by the keypress that chose a row, leaving no list to index by position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picking {
    Model,
    Thinking,
    Session {
        rows: Vec<(String, String)>,
    },
    /// The runs already put away. Taking one resumes it; Delete removes it for good.
    Archived {
        rows: Vec<(String, String)>,
    },
    /// What to clear for this directory. Taking a row asks before anything goes.
    Reset,
    /// The answer to that question, for the thing named.
    Resetting {
        what: String,
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

impl crate::app::App {
    /// Which command a chosen row means, and `None` for a list whose answer is not one: the
    /// picker is taken here, so the label is all there is to match a row back by.
    pub fn chose(&mut self, value: String) -> Option<UiCommand> {
        let app = self;
        Some(match app.picking.take() {
            Some(Picking::Thinking) => UiCommand::SetThinking { level: value },
            // No recorded purpose, so nothing here opened it and nothing goes.
            Some(Picking::Model) => UiCommand::SetModel { name: value },
            // Matched back by position: a row is labelled to be read, not by id.
            Some(Picking::Session { rows } | Picking::Archived { rows }) => {
                let found = rows
                    .iter()
                    .find(|(label, _)| *label == value)
                    .map(|(_, id)| id.clone());
                match found {
                    Some(id) => UiCommand::Resume { id },
                    None => return None,
                }
            }
            Some(Picking::Asked { id, rows }) => {
                let chosen = rows
                    .iter()
                    .find(|(label, _)| *label == value)
                    .map(|(_, choice)| choice.clone());
                match chosen {
                    Some(choice) => UiCommand::Answered { id, choice },
                    // No row matches: answering would resume with a choice nobody made.
                    None => return None,
                }
            }
            // Matched back by label, generated from these scopes, so the pairing is exact.
            Some(Picking::Permission { id, offers }) => {
                let chosen = offers
                    .iter()
                    .find(|scope| scope.label(&app.asking_about) == value);
                // The enforcing ledger is on the worker thread and never read back.
                if let Some(scope) = chosen
                    && let Some(grant) = magi_tools::permit::standing(&app.asking_about, scope)
                {
                    app.was_granted(grant);
                }
                let decision = chosen.map_or(magi_proto::permit::Decision::Deny, |scope| {
                    magi_proto::permit::Decision::Allow {
                        scope: scope.clone(),
                        lifetime: magi_proto::permit::Lifetime::Session,
                    }
                });
                UiCommand::Permit { id, decision }
            }
            // Answered here: one of the things cleared is what would carry a command.
            Some(Picking::Reset) => {
                app.confirm_reset(&value);
                return None;
            }
            Some(Picking::Resetting { what }) => {
                if value == "yes" {
                    app.reset_now(&what);
                }
                return None;
            }
            // Taken above: its answer is not a `UiCommand`.
            Some(Picking::Adoption { .. }) | None => return None,
        })
    }
}
