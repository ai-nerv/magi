//! Key handling.
//!
//! Shift+Enter for a newline requires the Kitty keyboard protocol; without it a terminal
//! reports both Enter and Shift+Enter identically and there is nothing to disambiguate.
//!
//! When something is open under the prompt it takes the navigation keys first, so Tab, the arrows,
//! Enter, and Escape mean "the popup" rather than "the prompt".

#[cfg(test)]
#[path = "keys/accept.rs"]
mod accept_tests;

mod modal;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use magi_tui::Editor;
use magi_tui::complete::{Completion, Kind};
pub use modal::Modal;

/// A movement of the transcript view.
///
/// Emitted in both backends. Inline mode has no owned buffer to move, so it lets the key
/// through to the terminal, whose own scrollback answers it — which is the point: the two
/// backends must not differ in what the user can do, only in who keeps the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Up one page.
    PageUp,
    /// Down one page.
    PageDown,
    /// To the first line.
    Top,
    /// To the newest output, resuming follow.
    Bottom,
    /// Up a few lines, for a mouse wheel.
    LineUp,
    /// Down a few lines, for a mouse wheel.
    LineDown,
}

/// What a keypress asks the driver to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// The buffer changed; redraw.
    Redraw,
    /// A completion was taken, and the popup must stay closed until the next keystroke.
    Accepted,
    /// The highlight moved inside an open list; the list itself must not be recomputed.
    ///
    /// **Its own action because `Redraw` was not specific enough.** The completion popup is
    /// derived from the prompt and rebuilt after every keystroke that is not one of these, and a
    /// rebuild starts at the first row. So Up and Down moved the highlight, the driver
    /// immediately recomputed the menu from a prompt that had not changed, and the selection went
    /// back to the top — the arrow keys did nothing at all, on every list `/` opened.
    ///
    /// The guard beside the rebuild already said "not while a list is open"; it tested only for
    /// the picker, which is the list the arrows *did* work in.
    Moved,
    /// A line came back from history, and the popup must not reopen over it.
    ///
    /// Its own action because recalling `:model` used to put the command menu back on screen,
    /// and the menu owns the arrow keys — so the next Up moved the highlight instead of
    /// reaching further back, and history stopped at the first slash command in it.
    Recalled,
    /// Show tool results in full, or fold them back.
    ToggleDetail,
    /// Take the mouse from the terminal for the wheel and for clicking, or give it back.
    ///
    /// A capture is all-or-nothing, and the terminal has it by default: dragging out a
    /// selection is what a terminal is for, and magi holding the mouse is the only thing that
    /// can stop it. This is the opt-in, and the footer says when it is on.
    /// A row was taken from an open selection list.
    Chose(String),
    /// A selection list was left without taking a row.
    ///
    /// Distinct from [`Action::Accepted`] because something may be waiting on the answer: a
    /// permission question closed with no reply leaves the turn that asked it blocked until it
    /// gives up on its own, which reads as a hang.
    Dismissed,
    /// Send this prompt.
    Submit(String),
    /// Run this colon command.
    Command(String),
    /// Interrupt the running turn.
    Interrupt,
    /// Hand the prompt to `$EDITOR`.
    ExternalEdit,
    /// Move the transcript view.
    Scroll(Scroll),
    /// Start a search of the transcript.
    Search,
    /// Go to the next or previous match.
    Match {
        /// Forwards through the transcript, or backwards.
        forward: bool,
    },
    /// Nothing happened.
    Ignore,
}

/// Whether the completion popup must be recomputed from the prompt after this action.
///
/// **One definition, because the driver and the key handler disagreeing is the bug this exists
/// for.** The popup is derived from the prompt, so a keystroke that changed the prompt has to
/// rebuild it — that is how typing narrows the menu. A keystroke that only moved the highlight
/// has not changed the prompt, and rebuilding starts the list at the top again.
///
/// That is precisely what happened: Up and Down returned [`Action::Redraw`], which is also what
/// typing returns, so the driver could not tell the two apart and recomputed after both. Every
/// piece worked alone — the highlight moved, the popup drew the row it was told to — and the
/// arrow keys did nothing on any list `/` opened.
#[must_use]
pub fn recomputes(action: &Action) -> bool {
    !matches!(
        action,
        Action::Accepted | Action::Dismissed | Action::Recalled | Action::Moved
    )
}
/// Apply a keypress to the editor and whatever is open under it.
///
/// `busy` gates submission: a prompt sent mid-turn would be a steering message, which is an
/// M2 concern, so for now Enter during a turn does nothing.
/// A selection list outranks the prompt for the navigation keys, and so does a completion popup,
/// for the same reason: while one is open it is what the arrows are about. They are one slot —
/// see [`magi_tui::overlay::Overlay`] — but not one block of handling, because quit and interrupt
/// go between them.
pub fn handle(
    key: KeyEvent,
    editor: &mut Editor,
    overlay: &mut Option<magi_tui::overlay::Overlay>,
    busy: bool,
    modal: &mut Modal,
) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    if let Some(open) = overlay
        .as_mut()
        .and_then(magi_tui::overlay::Overlay::picker)
    {
        match key.code {
            KeyCode::Esc => {
                // One escape closes the whole list, not one character of the query. Backspace
                // is how you widen it; escape is how you leave.
                *overlay = None;
                return Action::Dismissed;
            }
            // Typing narrows the list rather than reaching the prompt. Fifty-three rows is
            // more than anyone should arrow through, and the prompt is holding whatever it was
            // holding — this is not an edit of it.
            KeyCode::Char(c) if !ctrl && !alt => {
                open.push(c);
                return Action::Accepted;
            }
            KeyCode::Backspace => {
                // A query that has run out closes nothing: backspacing past the start is a
                // widened list, and leaving is what escape is for.
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
                // Only closes when something was actually taken. A row that cannot be used
                // says so and leaves the list up, because the next thing you want is a
                // different row and not to retype the query that found this one.
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

    // Normal mode, before anything that could take a character as text. Nothing below this
    // point is reached with a bare letter while the prompt is in normal mode, which is the
    // whole of what modal means: `i` is a command until it is told to be an `i`.
    if !modal.mode.is_insert() && overlay.is_none() {
        return modal::normal(key, editor, modal, busy);
    }

    // Quit and interrupt outrank the popup: a user reaching for them wants out, not a
    // dismissed menu they then have to escape from a second time.
    match key.code {
        // Neither of these leaves any more. `:q` is the way out, and a key that quits on an
        // empty prompt is a key that quits when you meant to clear one -- which is the same
        // keystroke, told apart only by what happened to be in the box.
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
                // At the top of the menu, Up leaves it for history rather than wrapping round
                // to the bottom. A menu that wraps is one you cannot walk out of, and typing
                // `/` put it between the user and every earlier prompt they had.
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
                // Not `Redraw`: the popup is recomputed from the prompt after every key, and
                // what was just accepted still matches what offered it. Saying so keeps the
                // caller from reopening the menu the user has this moment chosen from, which
                // left every exact-match command -- `:help`, `:quit` -- impossible to submit.
                return Action::Accepted;
            }
            KeyCode::Enter => {
                // Tab completes; Enter runs. A palette where enter only fills the box asks
                // for the key twice to reach one command, and reads as a menu that does
                // nothing -- which is what `:model` looked like for as long as this was
                // shared with Tab. A path completion is not a command, so there enter still
                // only completes: the line it belongs to is not finished yet.
                let command = open.kind == Kind::Command;
                accept(open, editor);
                *overlay = None;
                if !command {
                    return Action::Accepted;
                }
                // Through the command line, which has the prompt's own text put aside and
                // owes it back whether the command runs or not.
                return modal::finish_command(editor, modal);
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::PageUp => Action::Scroll(Scroll::PageUp),
        KeyCode::PageDown => Action::Scroll(Scroll::PageDown),
        // Shift is what separates "move the transcript" from "move within the prompt", which
        // is why Home and End alone stay line motions.
        KeyCode::Home if shift => Action::Scroll(Scroll::Top),
        KeyCode::End if shift => Action::Scroll(Scroll::Bottom),
        KeyCode::Up if shift => Action::Scroll(Scroll::LineUp),
        KeyCode::Down if shift => Action::Scroll(Scroll::LineDown),

        // Out of insert mode, and only that. Interrupting a turn is the *second* escape, from
        // normal mode, because a key that both left a mode and cancelled a turn would cancel
        // one every time somebody finished typing.
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
        // The command line runs whatever is on it, whether or not a turn is going: `:q` and
        // `:model` are the UI's business and do not wait on the daemon.
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
        // Backspacing the colon away is how vim leaves a command line, and the prompt gets its
        // text back the same as if escape had done it.
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
