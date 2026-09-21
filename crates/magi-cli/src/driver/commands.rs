//! The colon commands, and what running one does to the UI. Everything the UI can do without the
//! session is one arm here; anything that needs the session leaves as a [`UiCommand`] instead.

use crate::app::App;
use magi_proto::UiCommand;

/// Whether a slash command asked the UI to exit.
#[derive(Debug, PartialEq)]
pub(super) enum Control {
    Continue,
    Quit,
    /// Something only the session can do.
    Send(UiCommand),
}

/// Run a colon command locally, or turn it into a [`UiCommand`] for the session.
pub(super) fn run_command(input: &str, app: &mut App) -> Control {
    match input.split_whitespace().next().unwrap_or_default() {
        // `:q` and `:qa` do the same thing today: magi starts no subagents, and the `melchior serve`
        // beside it and the tool peers below it already go when this process goes.
        ":quit" | ":q" | ":quitall" | ":qa" => Control::Quit,
        // Clears both the view and what the model is shown, on whichever session is on screen; the
        // branch is journalled, so the record survives.
        ":clear" => {
            app.clear_view();
            Control::Send(UiCommand::Branch { keeps: Some(0) })
        }
        // Neither asks the daemon: the trace has been recorded since the session started, and what a
        // turn spent is on the entries already here.
        ":trace" => {
            app.show_trace();
            Control::Continue
        }
        ":cost" => {
            app.show_cost();
            Control::Continue
        }
        // As the session last reported it: balthasar lays each request out, and says so.
        ":context" => {
            app.show_context();
            Control::Continue
        }
        // balthasar's float, which the footer dot also opens: a control only the pointer can reach
        // is one nobody finds twice.
        ":memory" | ":balthasar" => {
            app.show_memory(0);
            match app.memory_opened() {
                Some(ask) => Control::Send(ask),
                None => Control::Continue,
            }
        }
        // Opened at once and filled when the memory layer answers; the notes are the session's to ask.
        ":notes" => {
            app.show_notes();
            Control::Send(UiCommand::Memory {
                verb: "notes".to_owned(),
                arg: serde_json::json!({}),
            })
        }
        // The session's to change: the gate is there, and every screen is told.
        ":mode" => Control::Send(UiCommand::SetMode {
            mode: input.split_whitespace().nth(1).unwrap_or("next").to_owned(),
        }),
        // What it shows is the session's, and arrives with every snapshot: opened here.
        ":permission" | ":perm" => {
            app.show_permission();
            Control::Continue
        }
        ":agents" => {
            app.show_agents();
            Control::Continue
        }
        ":help" => {
            app.show_help();
            Control::Continue
        }
        // With a name it is the session's to do: only it knows the catalog and whether the name
        // reaches anything.
        ":model" => match input.split_whitespace().nth(1) {
            Some(name) => Control::Send(UiCommand::SetModel {
                name: name.to_owned(),
            }),
            None => {
                app.open_model_picker();
                Control::Continue
            }
        },
        ":think" => match input.split_whitespace().nth(1) {
            Some(level) => Control::Send(UiCommand::SetThinking {
                level: level.to_owned(),
            }),
            None => {
                app.open_thinking_picker();
                Control::Continue
            }
        },
        ":permissions" => Control::Send(UiCommand::DeclareNeeds),
        ":resume" => {
            app.open_session_picker();
            Control::Continue
        }
        ":archives" | ":archive" => {
            app.open_archive_picker();
            Control::Continue
        }
        _ if input.split_whitespace().next() == Some(":rename") => {
            let name = input.trim_start()[":rename".len()..].trim();
            app.rename_session(name);
            Control::Continue
        }
        ":rewind" => match input.split_whitespace().nth(1) {
            None => Control::Send(UiCommand::Branch { keeps: None }),
            Some(n) => match n.parse() {
                Ok(keeps) => Control::Send(UiCommand::Branch { keeps: Some(keeps) }),
                Err(_) => {
                    app.show_notice(format!(":rewind takes a number, not {n:?}"));
                    Control::Continue
                }
            },
        },
        _ => {
            app.show_notice(format!("unknown command: {input}"));
            Control::Continue
        }
    }
}

/// Leaving, in both of vim's spellings.
#[cfg(test)]
mod quitting {
    use super::{Control, run_command};
    use crate::app::App;

    fn ran(input: &str) -> bool {
        matches!(run_command(input, &mut App::new()), Control::Quit)
    }

    #[test]
    fn both_spellings_leave() {
        for said in [":q", ":quit", ":qa", ":quitall"] {
            assert!(ran(said), "{said} did not leave");
        }
    }

    #[test]
    fn nothing_else_does() {
        for said in [":quite", ":qq", ":quitter", ":clear", "q", "quit", ""] {
            assert!(!ran(said), "{said:?} left");
        }
    }

    #[test]
    fn the_popup_offers_both() {
        // The help is built from this list, so a command offered here is a command documented there.
        let offered: Vec<String> = magi_tui::complete::commands()
            .into_iter()
            .map(|c| c.value)
            .collect();
        assert!(offered.iter().any(|c| c == ":quit"), "{offered:?}");
        assert!(offered.iter().any(|c| c == ":quitall"), "{offered:?}");
    }
}
