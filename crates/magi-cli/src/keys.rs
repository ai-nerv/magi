//! Key handling. Shift+Enter for a newline requires the Kitty keyboard protocol; without it a
//! terminal reports Enter and Shift+Enter identically. When something is open under the prompt it
//! takes the navigation keys first.

#[cfg(test)]
#[path = "keys/accept.rs"]
mod accept_tests;

mod modal;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use magi_tui::Editor;
use magi_tui::complete::{Completion, Kind};
pub use modal::Modal;

/// A movement of the transcript view, emitted in both backends: inline mode has no owned buffer, so
/// it lets the key through to the terminal's own scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    PageUp,
    PageDown,
    Top,
    Bottom,
    /// Up a few lines, for a mouse wheel.
    LineUp,
    /// Down a few lines, for a mouse wheel.
    LineDown,
}

/// What a keypress asks the driver to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Redraw,
    /// A completion was taken, and the popup must stay closed until the next keystroke.
    Accepted,
    /// The highlight moved inside an open list; the list itself must not be recomputed, because a
    /// rebuild starts at the first row.
    Moved,
    /// A line came back from history, and the popup must not reopen over it: the menu owns the
    /// arrow keys, so history would stop at the first slash command in it.
    Recalled,
    /// Show tool results in full, or fold them back.
    ToggleDetail,
    /// A row was taken from an open selection list.
    Chose(String),
    /// A selection list was left without taking a row. Distinct from [`Action::Accepted`] because a
    /// permission question closed with no reply leaves the turn that asked it blocked.
    Dismissed,
    Submit(String),
    Command(String),
    Interrupt,
    /// Point the screen at the next agent along, or the previous one. The two arrows in the footer
    /// are the only sign they exist, and a control you can only click is one nobody finds twice.
    Crew {
        /// Along the ring rather than back down it.
        forward: bool,
    },
    /// Hand the prompt to `$EDITOR`.
    ExternalEdit,
    Scroll(Scroll),
    Search,
    /// Go to the next or previous match.
    Match {
        forward: bool,
    },
    Ignore,
}

/// Whether the completion popup must be recomputed from the prompt after this action. One
/// definition, because a keystroke that only moved the highlight must not rebuild the list — which
/// is what made the arrow keys do nothing on every list `/` opened.
#[must_use]
pub fn recomputes(action: &Action) -> bool {
    !matches!(
        action,
        Action::Accepted | Action::Dismissed | Action::Recalled | Action::Moved
    )
}
/// Apply a keypress to the editor and whatever is open under it. `busy` gates submission. A
/// selection list and a completion popup each outrank the prompt for the navigation keys; they are
/// one slot but not one block of handling, because quit and interrupt go between them.
pub fn handle(
    key: KeyEvent,
    editor: &mut Editor,
    overlay: &mut Option<magi_tui::overlay::Overlay>,
    pane: &mut Option<magi_tui::pane::Pane>,
    page: usize,
    busy: bool,
    modal: &mut Modal,
) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    // A float takes the navigation keys first: escape closes it rather than clearing the prompt.
    if let Some(open) = pane.as_mut() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                *pane = None;
                return Action::Dismissed;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                open.up(1);
                return Action::Moved;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                open.down(1, page);
                return Action::Moved;
            }
            KeyCode::PageUp => {
                open.up(page);
                return Action::Moved;
            }
            KeyCode::PageDown => {
                open.down(page, page);
                return Action::Moved;
            }
            KeyCode::Home => {
                open.top = 0;
                return Action::Moved;
            }
            KeyCode::End => {
                open.bottom(page);
                return Action::Moved;
            }
            // Everything else is ignored rather than reaching the prompt: a float is modal.
            _ => return Action::Ignore,
        }
    }

    if let Some(open) = overlay
        .as_mut()
        .and_then(magi_tui::overlay::Overlay::picker)
    {
        match key.code {
            KeyCode::Esc => {
                // One escape closes the whole list, not one character of the query.
                *overlay = None;
                return Action::Dismissed;
            }
            // Typing narrows the list rather than reaching the prompt.
            KeyCode::Char(c) if !ctrl && !alt => {
                open.push(c);
                return Action::Accepted;
            }
            KeyCode::Backspace => {
                // Backspacing past the start is a widened list; leaving is what escape is for.
                open.pop();
                return Action::Accepted;
            }
            KeyCode::Up => {
                open.previous();
                return Action::Accepted;
            }
            KeyCode::Down => {
                open.next();
                return Action::Accepted;
            }
            KeyCode::Enter | KeyCode::Tab => {
                // Only closes when something was actually taken; a row that cannot be used leaves
                // the list up.
                return match open.take() {
                    Some(chosen) => {
                        *overlay = None;
                        Action::Chose(chosen)
                    }
                    None => Action::Accepted,
                };
            }
            _ => {}
        }
    }

    // Normal mode, before anything that could take a character as text: `i` is a command until it
    // is told to be an `i`.
    if !modal.mode.is_insert() && overlay.is_none() {
        return modal::normal(key, editor, modal, busy);
    }

    // Quit and interrupt outrank the popup: a user reaching for them wants out.
    match key.code {
        // Neither of these leaves any more: a key that quits on an empty prompt is a key that quits
        // when you meant to clear one.
        KeyCode::Char('c') if ctrl => {
            if overlay
                .as_ref()
                .is_some_and(magi_tui::overlay::Overlay::is_completion)
            {
                *overlay = None;
                return Action::Redraw;
            }
            editor.clear();
            return Action::Redraw;
        }
        KeyCode::Char('d') if ctrl && editor.is_blank() => return Action::Ignore,
        KeyCode::Char('x') if ctrl => return Action::ExternalEdit,
        KeyCode::Char('o') if ctrl => return Action::ToggleDetail,
        // The unshifted `<` and `>` keys, which the footer draws. Alt because the characters
        // themselves are text a prompt has to hold, and every other modifier is spoken for.
        KeyCode::Char(',') if alt => return Action::Crew { forward: false },
        KeyCode::Char('.') if alt => return Action::Crew { forward: true },
        _ => {}
    }

    if let Some(open) = overlay
        .as_mut()
        .and_then(magi_tui::overlay::Overlay::completion)
    {
        match key.code {
            KeyCode::Esc => {
                *overlay = None;
                return Action::Redraw;
            }
            KeyCode::Up => {
                // At the top of the menu, Up leaves it for history rather than wrapping round.
                if open.selected == 0 {
                    *overlay = None;
                    editor.history_prev();
                    return Action::Recalled;
                }
                open.prev();
                return Action::Moved;
            }
            KeyCode::Down => {
                open.next();
                return Action::Moved;
            }
            KeyCode::Tab => {
                accept(open, editor);
                *overlay = None;
                // Not `Redraw`: the popup is recomputed from the prompt after every key, so saying
                // so keeps the caller from reopening the menu just chosen from.
                return Action::Accepted;
            }
            KeyCode::Enter => {
                // Tab completes; Enter runs. A path completion is not a command, so there enter
                // only completes: the line it belongs to is not finished yet.
                let command = open.kind == Kind::Command;
                accept(open, editor);
                *overlay = None;
                if !command {
                    return Action::Accepted;
                }
                // Through the command line, which owes the prompt's text back either way.
                return modal::finish_command(editor, modal);
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::PageUp => Action::Scroll(Scroll::PageUp),
        KeyCode::PageDown => Action::Scroll(Scroll::PageDown),
        // Shift separates "move the transcript" from "move within the prompt".
        KeyCode::Home if shift => Action::Scroll(Scroll::Top),
        KeyCode::End if shift => Action::Scroll(Scroll::Bottom),
        KeyCode::Up if shift => Action::Scroll(Scroll::LineUp),
        KeyCode::Down if shift => Action::Scroll(Scroll::LineDown),

        // Out of insert mode, and only that: interrupting a turn is the *second* escape.
        KeyCode::Esc if modal.commanding() => {
            modal.close_command(editor);
            Action::Redraw
        }
        KeyCode::Esc => {
            modal.normal(editor);
            Action::Redraw
        }

        KeyCode::Enter if shift => {
            editor.newline();
            Action::Redraw
        }
        // The command line runs whether or not a turn is going: `:q` does not wait on the daemon.
        KeyCode::Enter if modal.commanding() => modal::finish_command(editor, modal),
        KeyCode::Enter => {
            if busy {
                return Action::Ignore;
            }
            match editor.submit() {
                Some(text) if text.starts_with(':') => Action::Command(text),
                Some(text) => Action::Submit(text),
                None => Action::Ignore,
            }
        }
        // Backspacing the colon away is how vim leaves a command line, and the prompt is restored.
        KeyCode::Backspace if modal.commanding() && editor.text() == ":" => {
            modal.close_command(editor);
            Action::Redraw
        }

        KeyCode::Backspace => {
            editor.backspace();
            Action::Redraw
        }
        KeyCode::Left if alt => {
            editor.word_left();
            Action::Redraw
        }
        KeyCode::Right if alt => {
            editor.word_right();
            Action::Redraw
        }
        KeyCode::Left => {
            editor.left();
            Action::Redraw
        }
        KeyCode::Right => {
            editor.right();
            Action::Redraw
        }
        KeyCode::Up => {
            editor.history_prev();
            Action::Recalled
        }
        KeyCode::Down => {
            editor.history_next();
            Action::Recalled
        }
        KeyCode::Home => {
            editor.home();
            Action::Redraw
        }
        KeyCode::End => {
            editor.end();
            Action::Redraw
        }

        KeyCode::Char('a') if ctrl => {
            editor.home();
            Action::Redraw
        }
        KeyCode::Char('e') if ctrl => {
            editor.end();
            Action::Redraw
        }
        KeyCode::Char('k') if ctrl => {
            editor.kill_to_end();
            Action::Redraw
        }
        KeyCode::Char('u') if ctrl => {
            editor.kill_to_start();
            Action::Redraw
        }
        KeyCode::Char('y') if ctrl => {
            editor.yank();
            Action::Redraw
        }

        KeyCode::Char(c) if !ctrl && !alt => {
            editor.insert(c);
            Action::Redraw
        }
        _ => Action::Ignore,
    }
}

/// Put the highlighted candidate into the prompt, replacing the token that offered it.
fn accept(open: &Completion, editor: &mut Editor) {
    if let Some(candidate) = open.current() {
        let value = candidate.value.clone();
        let start = open.token_start;
        editor.replace_token(start, &value);
    }
}
#[cfg(test)]
#[path = "keys/tests.rs"]
pub(super) mod tests;

#[cfg(test)]
mod history;

/// Walking an open list, and the rebuild that used to undo it.
#[cfg(test)]
#[path = "keys/moving.rs"]
mod moving;
