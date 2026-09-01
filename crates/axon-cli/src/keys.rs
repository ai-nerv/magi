//! Key handling.
//!
//! Shift+Enter for a newline requires the Kitty keyboard protocol; without it a terminal
//! reports both Enter and Shift+Enter identically and there is nothing to disambiguate.
//!
//! When something is open under the prompt it takes the navigation keys first, so Tab, the arrows,
//! Enter, and Escape mean "the popup" rather than "the prompt".

mod modal;

use axon_tui::Editor;
use axon_tui::complete::{Completion, Kind};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
    /// selection is what a terminal is for, and axon holding the mouse is the only thing that
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

/// Apply a keypress to the editor and whatever is open under it.
///
/// `busy` gates submission: a prompt sent mid-turn would be a steering message, which is an
/// M2 concern, so for now Enter during a turn does nothing.
/// A selection list outranks the prompt for the navigation keys, and so does a completion popup,
/// for the same reason: while one is open it is what the arrows are about. They are one slot —
/// see [`axon_tui::overlay::Overlay`] — but not one block of handling, because quit and interrupt
/// go between them.
pub fn handle(
    key: KeyEvent,
    editor: &mut Editor,
    overlay: &mut Option<axon_tui::overlay::Overlay>,
    busy: bool,
    modal: &mut Modal,
) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    if let Some(open) = overlay
        .as_mut()
        .and_then(axon_tui::overlay::Overlay::picker)
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
                .is_some_and(axon_tui::overlay::Overlay::is_completion)
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
        .and_then(axon_tui::overlay::Overlay::completion)
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
                return Action::Redraw;
            }
            KeyCode::Down => {
                open.next();
                return Action::Redraw;
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
                if !command || busy {
                    return Action::Accepted;
                }
                return match editor.submit() {
                    Some(text) if text.starts_with(':') => Action::Command(text),
                    Some(text) => Action::Submit(text),
                    None => Action::Accepted,
                };
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
        KeyCode::Esc => {
            modal.normal(editor);
            Action::Redraw
        }

        KeyCode::Enter if shift => {
            editor.newline();
            Action::Redraw
        }
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
pub(super) mod tests {
    use super::*;
    use axon_tui::complete;
    use axon_tui::vim::Mode;

    pub(super) fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    pub(super) fn no_paths(_: &str) -> Vec<String> {
        Vec::new()
    }

    fn act(key: KeyEvent, editor: &mut Editor, busy: bool) -> Action {
        handle(key, editor, &mut None, busy, &mut typing())
    }

    /// A prompt already in insert mode, which is what everything below the modal tests is about.
    pub(super) fn typing() -> Modal {
        let mut modal = Modal::default();
        modal.insert();
        modal
    }

    /// An editor holding `text`, with the completion popup its content would open.
    pub(super) fn with_popup(text: &str) -> (Editor, Option<axon_tui::overlay::Overlay>) {
        let mut editor = Editor::new();
        editor.insert_str(text);
        let (_, col) = editor.cursor();
        let line = editor.lines()[0].clone();
        (
            editor,
            complete::resolve(&line, col, &no_paths).map(Into::into),
        )
    }

    #[test]
    fn typing_inserts() {
        let mut editor = Editor::new();
        assert_eq!(
            act(
                press(KeyCode::Char('a'), KeyModifiers::NONE),
                &mut editor,
                false
            ),
            Action::Redraw
        );
        assert_eq!(editor.text(), "a");
    }

    #[test]
    fn enter_submits_when_idle() {
        let mut editor = Editor::new();
        editor.insert_str("go");
        assert_eq!(
            act(
                press(KeyCode::Enter, KeyModifiers::NONE),
                &mut editor,
                false
            ),
            Action::Submit("go".into())
        );
    }

    #[test]
    fn a_slash_prefixed_prompt_submits_as_a_command() {
        let mut editor = Editor::new();
        editor.insert_str(":quit");
        assert_eq!(
            act(
                press(KeyCode::Enter, KeyModifiers::NONE),
                &mut editor,
                false
            ),
            Action::Command(":quit".into())
        );
    }

    #[test]
    fn enter_during_a_turn_does_nothing() {
        let mut editor = Editor::new();
        editor.insert_str("go");
        assert_eq!(
            act(press(KeyCode::Enter, KeyModifiers::NONE), &mut editor, true),
            Action::Ignore
        );
        assert_eq!(editor.text(), "go", "the buffer survives");
    }

    #[test]
    fn shift_enter_inserts_a_newline_instead_of_submitting() {
        let mut editor = Editor::new();
        editor.insert_str("a");
        assert_eq!(
            act(
                press(KeyCode::Enter, KeyModifiers::SHIFT),
                &mut editor,
                false
            ),
            Action::Redraw
        );
        assert_eq!(editor.text(), "a\n");
    }

    #[test]
    fn escape_leaves_insert_mode_before_it_interrupts_anything() {
        // Two escapes, and they mean different things. A key that both left a mode and
        // cancelled a turn would cancel one every time somebody finished typing a sentence.
        let mut editor = Editor::new();
        let mut modal = typing();
        assert_eq!(
            handle(
                press(KeyCode::Esc, KeyModifiers::NONE),
                &mut editor,
                &mut None,
                true,
                &mut modal,
            ),
            Action::Redraw,
            "the first one only leaves insert mode"
        );
        assert_eq!(modal.mode, Mode::Normal);
        assert_eq!(
            handle(
                press(KeyCode::Esc, KeyModifiers::NONE),
                &mut editor,
                &mut None,
                true,
                &mut modal,
            ),
            Action::Interrupt,
            "and the second one, from normal mode, interrupts"
        );
        assert_eq!(
            handle(
                press(KeyCode::Esc, KeyModifiers::NONE),
                &mut editor,
                &mut None,
                false,
                &mut modal,
            ),
            Action::Redraw,
            "with nothing running there is nothing to interrupt"
        );
    }
    #[test]
    fn ctrl_c_clears_the_buffer_and_never_leaves() {
        // It used to quit on an empty prompt, which is the same keystroke as clearing one and
        // told apart only by what happened to be in the box. `:q` is the way out now.
        let mut editor = Editor::new();
        editor.insert_str("draft");
        assert_eq!(
            act(
                press(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &mut editor,
                false
            ),
            Action::Redraw
        );
        assert_eq!(editor.text(), "");
        assert_eq!(
            act(
                press(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &mut editor,
                false
            ),
            Action::Redraw,
            "a second one on an empty prompt still does not leave"
        );
        assert_eq!(
            act(
                press(KeyCode::Char('d'), KeyModifiers::CONTROL),
                &mut editor,
                false
            ),
            Action::Ignore,
            "and neither does ctrl+d"
        );
    }
    #[test]
    fn ctrl_x_opens_the_external_editor() {
        let mut editor = Editor::new();
        assert_eq!(
            act(
                press(KeyCode::Char('x'), KeyModifiers::CONTROL),
                &mut editor,
                false
            ),
            Action::ExternalEdit
        );
    }

    #[test]
    fn readline_bindings_move_and_kill() {
        let mut editor = Editor::new();
        editor.insert_str("hello");
        act(
            press(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &mut editor,
            false,
        );
        assert_eq!(editor.cursor(), (0, 0));
        act(
            press(KeyCode::Char('k'), KeyModifiers::CONTROL),
            &mut editor,
            false,
        );
        assert_eq!(editor.text(), "");
        act(
            press(KeyCode::Char('y'), KeyModifiers::CONTROL),
            &mut editor,
            false,
        );
        assert_eq!(editor.text(), "hello");
    }

    #[test]
    fn page_keys_move_the_transcript() {
        let mut editor = Editor::new();
        assert_eq!(
            act(
                press(KeyCode::PageUp, KeyModifiers::NONE),
                &mut editor,
                false
            ),
            Action::Scroll(Scroll::PageUp)
        );
        assert_eq!(
            act(
                press(KeyCode::PageDown, KeyModifiers::NONE),
                &mut editor,
                false
            ),
            Action::Scroll(Scroll::PageDown)
        );
    }

    #[test]
    fn shift_separates_transcript_motion_from_prompt_motion() {
        let mut editor = Editor::new();
        editor.insert_str("hello");
        assert_eq!(
            act(press(KeyCode::Home, KeyModifiers::NONE), &mut editor, false),
            Action::Redraw,
            "plain Home is a line motion"
        );
        assert_eq!(editor.cursor(), (0, 0));
        assert_eq!(
            act(
                press(KeyCode::Home, KeyModifiers::SHIFT),
                &mut editor,
                false
            ),
            Action::Scroll(Scroll::Top),
            "Shift+Home scrolls the transcript"
        );
    }

    #[test]
    fn shift_arrows_scroll_without_touching_prompt_history() {
        let mut editor = Editor::new();
        editor.insert_str("draft");
        editor.submit();
        assert_eq!(
            act(press(KeyCode::Up, KeyModifiers::SHIFT), &mut editor, false),
            Action::Scroll(Scroll::LineUp)
        );
        assert_eq!(editor.text(), "", "prompt history did not move");
    }

    #[test]
    fn tab_accepts_the_highlighted_completion() {
        let (mut editor, mut popup) = with_popup(":qu");
        assert!(popup.is_some(), "a colon query opens the palette");
        handle(
            press(KeyCode::Tab, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            false,
            &mut typing(),
        );
        assert_eq!(editor.text(), ":quit");
        assert!(popup.is_none(), "accepting closes the popup");
    }

    #[test]
    fn enter_runs_the_command_the_palette_offered() {
        // Two presses to reach one command reads as a palette that does nothing, which is
        // what `:model` looked like for as long as enter merely filled the box.
        let (mut editor, mut popup) = with_popup(":qu");
        let action = handle(
            press(KeyCode::Enter, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            false,
            &mut typing(),
        );
        assert_eq!(action, Action::Command(":quit".into()));
        assert_eq!(editor.text(), "", "and the prompt is spent");
    }

    #[test]
    fn enter_on_a_path_completion_only_completes() {
        // A path is part of a sentence, not the whole of one: the line it belongs to is not
        // finished, so submitting it would send half a thought.
        let mut editor = Editor::new();
        editor.insert_str("look at @Car");
        let (_, col) = editor.cursor();
        let line = editor.lines()[0].clone();
        let mut popup: Option<axon_tui::overlay::Overlay> =
            complete::resolve(&line, col, &no_paths).map(Into::into);
        if popup.is_none() {
            return;
        }
        let action = handle(
            press(KeyCode::Enter, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            false,
            &mut typing(),
        );
        assert_eq!(action, Action::Accepted, "completed, not submitted");
    }

    #[test]
    fn the_arrows_move_the_highlight_while_a_popup_is_open() {
        let (mut editor, mut popup) = with_popup(":");
        handle(
            press(KeyCode::Down, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            false,
            &mut typing(),
        );
        assert_eq!(
            popup
                .as_mut()
                .and_then(axon_tui::overlay::Overlay::completion)
                .map(|p| p.selected),
            Some(1)
        );
        assert_eq!(editor.text(), ":", "history did not move the buffer");
    }

    #[test]
    fn escape_dismisses_a_popup_before_it_interrupts() {
        let (mut editor, mut popup) = with_popup(":");
        let action = handle(
            press(KeyCode::Esc, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            true,
            &mut typing(),
        );
        assert_eq!(action, Action::Redraw);
        assert!(popup.is_none());
    }

    #[test]
    fn escape_out_of_a_list_says_so_rather_than_going_quiet() {
        // Something may be waiting on the answer, and a list closed with `Accepted` told
        // nobody: the turn that asked stayed blocked until its own patience ran out.
        let mut editor = Editor::new();
        let mut overlay = Some(
            axon_tui::picker::Picker::new(
                "read wants to read /etc/hosts",
                vec![axon_tui::picker::Choice {
                    value: "just this once".to_owned(),
                    detail: String::new(),
                    ready: true,
                }],
                None,
            )
            .into(),
        );
        let action = handle(
            press(KeyCode::Esc, KeyModifiers::NONE),
            &mut editor,
            &mut overlay,
            true,
            &mut typing(),
        );
        assert_eq!(action, Action::Dismissed);
        assert!(overlay.is_none());
    }

    #[test]
    fn ctrl_c_dismisses_a_popup_before_it_clears_the_buffer() {
        let (mut editor, mut popup) = with_popup(":qu");
        handle(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut editor,
            &mut popup,
            false,
            &mut typing(),
        );
        assert!(popup.is_none());
        assert_eq!(editor.text(), ":qu", "the buffer is untouched");
    }
}

#[cfg(test)]
mod accept_tests {
    use super::tests::typing;
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn tab_taking_a_completion_says_so_rather_than_asking_for_a_redraw() {
        // The driver recomputes the popup after every key. Told only "redraw", it reopens the
        // menu on the thing just chosen from it -- and `:help`, whose name is exactly what
        // offered it, can then never be submitted at all.
        let mut editor = Editor::new();
        editor.insert_str(":hel");
        let mut overlay: Option<axon_tui::overlay::Overlay> =
            axon_tui::complete::resolve(":hel", 4, &|_| Vec::new()).map(Into::into);
        assert!(overlay.is_some(), "the popup is open");

        let action = handle(
            press(KeyCode::Tab),
            &mut editor,
            &mut overlay,
            false,
            &mut typing(),
        );
        assert_eq!(action, Action::Accepted);
        assert!(overlay.is_none(), "and closed");
        assert_eq!(editor.text(), ":help");
    }

    #[test]
    fn enter_after_a_tab_submits_what_was_taken() {
        let mut editor = Editor::new();
        editor.insert_str(":help");
        let action = handle(
            press(KeyCode::Enter),
            &mut editor,
            &mut None,
            false,
            &mut typing(),
        );
        assert_eq!(action, Action::Command(":help".to_owned()));
    }
}

#[cfg(test)]
mod history;
