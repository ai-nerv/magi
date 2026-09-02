//! The colon commands, and what running one does to the UI.
//!
//! Its own file because it is a closed list: everything the UI can do without the session
//! is one arm here, and anything that needs the session leaves as a [`UiCommand`] instead.
//! Keeping the two apart in one match is what makes the boundary readable.

use crate::app::App;
use axon_proto::UiCommand;

/// Whether a slash command asked the UI to exit.
#[derive(Debug, PartialEq)]
pub(super) enum Control {
    /// Stay running.
    Continue,
    /// Exit.
    Quit,
    /// Something only the session can do.
    Send(UiCommand),
}

/// Run a colon command.
///
/// Every command here is answered locally. Anything that needs the session becomes a
/// [`UiCommand`] instead, so the set of things the UI can do alone stays visible in one match.
pub(super) fn run_command(input: &str, app: &mut App) -> Control {
    match input.split_whitespace().next().unwrap_or_default() {
        // vim's spellings, both of them. `:q` closes this window and `:qa` closes every window —
        // and a person who has typed `:qa` for twenty years should not have to find out here
        // that only one of the two was wired.
        //
        // They do the same thing today because a session has nothing under it: axon starts no
        // subagents yet, and the `atom serve` beside it and the tool peers below it already go
        // when this process goes. `:qa` is the spelling that will mean "and everything this
        // session started" once there is something to start.
        ":quit" | ":q" | ":quitall" | ":qa" => Control::Quit,
        // Both halves, because the name promises both. Clearing only the view left the model
        // remembering everything while the footer reported an empty context -- the screen and
        // the token count both lying, in the same direction, at the same time. The branch is
        // journalled, so the record of what was said survives what the model is shown.
        ":clear" => {
            app.clear_view();
            Control::Send(UiCommand::Branch { keeps: Some(0) })
        }
        ":help" => {
            app.show_help();
            Control::Continue
        }
        // With a name it is the session's to do: only it knows the catalog this session
        // started with and whether the name reaches anything. Without one, the answer is
        // already on screen — the footer says which model is answering — so this says it
        // again in words, which is what somebody typing `:model` is asking for.
        ":model" => match input.split_whitespace().nth(1) {
            Some(name) => Control::Send(UiCommand::SetModel {
                name: name.to_owned(),
            }),
            // A list rather than a sentence. Somebody asking this has usually configured
            // nothing, and being told "no model is configured" answers the question they did
            // not ask while leaving the one they did.
            None => {
                app.open_model_picker();
                Control::Continue
            }
        },
        // Same shape as `:model`: a list rather than a sentence, because the useful reply to
        // "how much reasoning" is the set of answers and which of them this model can give.
        ":think" => match input.split_whitespace().nth(1) {
            Some(level) => Control::Send(UiCommand::SetThinking {
                level: level.to_owned(),
            }),
            None => {
                app.open_thinking_picker();
                Control::Continue
            }
        },
        // The session's, because it holds the conversation the question is about and the
        // provider that answers it.
        ":permissions" => Control::Send(UiCommand::DeclareNeeds),
        // A list rather than a flag. `--resume` continues this directory's most recent session
        // and there was no way to reach any of the others, which is most of them.
        ":resume" => {
            app.open_session_picker();
            Control::Continue
        }
        // Rewinding is the session's to work out: it holds the session, and which messages are
        // still live is a question about the session rather than about what is on screen.
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
        // A person who has typed `:qa` for twenty years should not discover here that only one
        // of the two was wired — and "nothing happened" is what an unknown command looks like.
        for said in [":q", ":quit", ":qa", ":quitall"] {
            assert!(ran(said), "{said} did not leave");
        }
    }

    #[test]
    fn nothing_else_does() {
        // The reason `:q` exists at all: leaving should take a deliberate word, not a stray key.
        for said in [":quite", ":qq", ":quitter", ":clear", "q", "quit", ""] {
            assert!(!ran(said), "{said:?} left");
        }
    }

    #[test]
    fn the_popup_offers_both() {
        // The help is built from this list, so a command offered here is a command documented
        // there — the pair has drifted before.
        let offered: Vec<String> = axon_tui::complete::commands()
            .into_iter()
            .map(|c| c.value)
            .collect();
        assert!(offered.iter().any(|c| c == ":quit"), "{offered:?}");
        assert!(offered.iter().any(|c| c == ":quitall"), "{offered:?}");
    }
}
