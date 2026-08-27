//! Key handling.
//!
//! Shift+Enter for a newline requires the Kitty keyboard protocol; without it a terminal
//! reports both Enter and Shift+Enter identically and there is nothing to disambiguate.
//!
//! When a completion popup is open it takes the navigation keys first, so Tab, the arrows,
//! Enter, and Escape mean "the popup" rather than "the prompt".

use axum_tui::Editor;
use axum_tui::complete::Completion;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    /// Send this prompt.
    Submit(String),
    /// Run this slash command.
    Command(String),
    /// Interrupt the running turn.
    Interrupt,
    /// Hand the prompt to `$EDITOR`.
    ExternalEdit,
    /// Move the transcript view.
    Scroll(Scroll),
    /// Leave.
    Quit,
    /// Nothing happened.
    Ignore,
}

/// Apply a keypress to the editor and any open completion.
///
/// `busy` gates submission: a prompt sent mid-turn would be a steering message, which is an
/// M2 concern, so for now Enter during a turn does nothing.
pub fn handle(
    key: KeyEvent,
    editor: &mut Editor,
    completion: &mut Option<Completion>,
    busy: bool,
) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    // Quit and interrupt outrank the popup: a user reaching for them wants out, not a
    // dismissed menu they then have to escape from a second time.
    match key.code {
        KeyCode::Char('c') if ctrl => {
            if completion.take().is_some() {
                return Action::Redraw;
            }
            if editor.is_blank() {
                return Action::Quit;
            }
            editor.clear();
            return Action::Redraw;
        }
        KeyCode::Char('d') if ctrl && editor.is_blank() => return Action::Quit,
        KeyCode::Char('x') if ctrl => return Action::ExternalEdit,
        _ => {}
    }

    if let Some(open) = completion.as_mut() {
        match key.code {
            KeyCode::Esc => {
                *completion = None;
                return Action::Redraw;
            }
            KeyCode::Up => {
                open.prev();
                return Action::Redraw;
            }
            KeyCode::Down => {
                open.next();
                return Action::Redraw;
            }
            KeyCode::Tab | KeyCode::Enter => {
                if let Some(candidate) = open.current() {
                    let value = candidate.value.clone();
                    let start = open.token_start;
                    editor.replace_token(start, &value);
                }
                *completion = None;
                return Action::Redraw;
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

        KeyCode::Esc if busy => Action::Interrupt,

        KeyCode::Enter if shift => {
            editor.newline();
            Action::Redraw
        }
        KeyCode::Enter => {
            if busy {
                return Action::Ignore;
            }
            match editor.submit() {
                Some(text) if text.starts_with('/') => Action::Command(text),
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
            Action::Redraw
        }
        KeyCode::Down => {
            editor.history_next();
            Action::Redraw
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum_tui::complete;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn no_paths(_: &str) -> Vec<String> {
        Vec::new()
    }

    fn act(key: KeyEvent, editor: &mut Editor, busy: bool) -> Action {
        handle(key, editor, &mut None, busy)
    }

    /// An editor holding `text`, with the completion popup its content would open.
    fn with_popup(text: &str) -> (Editor, Option<Completion>) {
        let mut editor = Editor::new();
        editor.insert_str(text);
        let (_, col) = editor.cursor();
        let line = editor.lines()[0].clone();
        (editor, complete::resolve(&line, col, &no_paths))
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
        editor.insert_str("/quit");
        assert_eq!(
            act(
                press(KeyCode::Enter, KeyModifiers::NONE),
                &mut editor,
                false
            ),
            Action::Command("/quit".into())
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
    fn escape_interrupts_only_while_busy() {
        let mut editor = Editor::new();
        assert_eq!(
            act(press(KeyCode::Esc, KeyModifiers::NONE), &mut editor, true),
            Action::Interrupt
        );
        assert_eq!(
            act(press(KeyCode::Esc, KeyModifiers::NONE), &mut editor, false),
            Action::Ignore
        );
    }

    #[test]
    fn ctrl_c_clears_a_full_buffer_and_quits_an_empty_one() {
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
            Action::Quit
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
        let (mut editor, mut popup) = with_popup("/qu");
        assert!(popup.is_some(), "a slash query opens the palette");
        handle(
            press(KeyCode::Tab, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            false,
        );
        assert_eq!(editor.text(), "/quit");
        assert!(popup.is_none(), "accepting closes the popup");
    }

    #[test]
    fn enter_accepts_a_completion_rather_than_submitting() {
        let (mut editor, mut popup) = with_popup("/qu");
        let action = handle(
            press(KeyCode::Enter, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            false,
        );
        assert_eq!(action, Action::Redraw, "the prompt is not submitted");
        assert_eq!(editor.text(), "/quit");
    }

    #[test]
    fn the_arrows_move_the_highlight_while_a_popup_is_open() {
        let (mut editor, mut popup) = with_popup("/");
        handle(
            press(KeyCode::Down, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            false,
        );
        assert_eq!(popup.as_ref().map(|p| p.selected), Some(1));
        assert_eq!(editor.text(), "/", "history did not move the buffer");
    }

    #[test]
    fn escape_dismisses_a_popup_before_it_interrupts() {
        let (mut editor, mut popup) = with_popup("/");
        let action = handle(
            press(KeyCode::Esc, KeyModifiers::NONE),
            &mut editor,
            &mut popup,
            true,
        );
        assert_eq!(action, Action::Redraw);
        assert!(popup.is_none());
    }

    #[test]
    fn ctrl_c_dismisses_a_popup_before_it_clears_the_buffer() {
        let (mut editor, mut popup) = with_popup("/qu");
        handle(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut editor,
            &mut popup,
            false,
        );
        assert!(popup.is_none());
        assert_eq!(editor.text(), "/qu", "the buffer is untouched");
    }
}
