//! Normal mode.
//!
//! The prompt opens here and stays here until something asks for insert mode. Every key is a
//! command, and the one thing none of them do is put themselves in the buffer — which is the
//! whole of what modal means, and the part that was missing when the modes were first added.
//!
//! Split from [`super`] because the two halves answer different questions: that file decides
//! who owns a keystroke — an open list, the prompt, the session — and this one decides what a
//! key means once the prompt has it and is not taking text.

use super::{Action, Scroll};
use axon_tui::Editor;
use axon_tui::vim::{self, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The prompt's mode, and whatever half-typed command is waiting on its second key.
///
/// Held by the caller rather than in a static, because it is per-prompt state and because a
/// half-typed `d` has to be dropped when anything else touches the buffer.
#[derive(Debug, Default)]
pub struct Modal {
    /// Which mode the prompt is in.
    pub mode: Mode,
    /// The first key of a two-key command, if one has been pressed.
    pending: Option<char>,
}

impl Modal {
    /// Go to insert mode.
    pub fn insert(&mut self) {
        self.mode = Mode::Insert;
        self.pending = None;
    }

    /// Go to normal mode, pulling the cursor back onto a character.
    ///
    /// Insert mode legitimately rests one past the end of the line and normal mode sits *on* a
    /// character, so leaving without this puts the block cursor in a column that has nothing
    /// in it — and every subsequent `x` deletes one character to the left of where it looks.
    pub fn normal(&mut self, editor: &mut Editor) {
        self.mode = Mode::Normal;
        self.pending = None;
        editor.settle();
    }
}

/// One key in normal mode.
///
/// Every path out of here either does something or does nothing. What it never does is put the
/// key in the buffer: that is what insert mode is for, and reaching it is a command like any
/// other.
pub(super) fn normal(key: KeyEvent, editor: &mut Editor, modal: &mut Modal, busy: bool) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let waiting = modal.pending.take();

    // The keys that mean the same thing in both modes, so muscle memory built anywhere else
    // still works. Arrows especially: a modal prompt that will not take an arrow key is being
    // difficult for the sake of it.
    match key.code {
        KeyCode::Esc if busy => return Action::Interrupt,
        KeyCode::Esc => return Action::Redraw,
        KeyCode::Left => {
            editor.left();
            return Action::Redraw;
        }
        KeyCode::Right => {
            editor.right();
            return Action::Redraw;
        }
        KeyCode::Up if editor.height() > 1 => {
            editor.up();
            return Action::Redraw;
        }
        KeyCode::Down if editor.height() > 1 => {
            editor.down();
            return Action::Redraw;
        }
        KeyCode::Up => return Action::Scroll(Scroll::LineUp),
        KeyCode::Down => return Action::Scroll(Scroll::LineDown),
        KeyCode::Home => {
            editor.home();
            return Action::Redraw;
        }
        KeyCode::End => {
            editor.end();
            return Action::Redraw;
        }
        KeyCode::PageUp => return Action::Scroll(Scroll::PageUp),
        KeyCode::PageDown => return Action::Scroll(Scroll::PageDown),
        KeyCode::Char('c') if ctrl => {
            editor.clear();
            return Action::Redraw;
        }
        KeyCode::Char('x') if ctrl => return Action::ExternalEdit,
        KeyCode::Char('o') if ctrl => return Action::ToggleDetail,
        KeyCode::Char('d') if ctrl => return Action::Scroll(Scroll::PageDown),
        KeyCode::Char('u') if ctrl => return Action::Scroll(Scroll::PageUp),
        KeyCode::Enter => {
            if busy {
                return Action::Ignore;
            }
            return match editor.submit() {
                Some(text) if text.starts_with(':') => Action::Command(text),
                Some(text) => Action::Submit(text),
                None => Action::Ignore,
            };
        }
        _ => {}
    }

    let KeyCode::Char(c) = key.code else {
        return Action::Ignore;
    };
    if ctrl || key.modifiers.contains(KeyModifiers::ALT) {
        return Action::Ignore;
    }
    // Held rather than acted on, and only when nothing is already held: `ddd` is `dd` and then
    // a `d` that starts another, not three quarters of two deletes.
    if waiting.is_none() && vim::holds(c) {
        modal.pending = Some(c);
        return Action::Redraw;
    }

    match vim::deed(c, waiting, editor.height() > 1) {
        vim::Deed::Edit(edit) => {
            vim::apply(edit, editor);
            // Back onto a character, because every edit here can leave the cursor past the end
            // of a line it just shortened.
            editor.settle();
            Action::Redraw
        }
        vim::Deed::Insert(first) => {
            if let Some(edit) = first {
                vim::apply(edit, editor);
            }
            modal.insert();
            Action::Redraw
        }
        vim::Deed::Command => {
            modal.insert();
            editor.insert(':');
            Action::Redraw
        }
        vim::Deed::Search => Action::Search,
        vim::Deed::Match { forward } => Action::Match { forward },
        vim::Deed::Scroll(toward) => Action::Scroll(match toward {
            vim::Toward::HalfUp => Scroll::PageUp,
            vim::Toward::HalfDown => Scroll::PageDown,
            vim::Toward::LineUp => Scroll::LineUp,
            vim::Toward::LineDown => Scroll::LineDown,
            vim::Toward::Top => Scroll::Top,
            vim::Toward::Bottom => Scroll::Bottom,
        }),
        vim::Deed::Submit => match editor.submit() {
            Some(text) if text.starts_with(':') => Action::Command(text),
            Some(text) => Action::Submit(text),
            None => Action::Ignore,
        },
        vim::Deed::Unbound => Action::Ignore,
    }
}
/// The prompt is modal: a letter is a command until you say otherwise.
#[cfg(test)]
mod modal_tests {
    use super::super::handle;
    use axon_tui::vim::Mode;

    use super::super::tests::typing;
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Send `keys` to a fresh prompt in normal mode, and give back what it holds.
    fn keyed(keys: &str) -> (Editor, Modal) {
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        for c in keys.chars() {
            handle(
                press(KeyCode::Char(c)),
                &mut editor,
                &mut None,
                false,
                &mut modal,
            );
        }
        (editor, modal)
    }

    #[test]
    fn nothing_typed_in_normal_mode_reaches_the_buffer() {
        // The whole point, and the thing that was missing: every letter is a command, and a
        // prompt that quietly took them was not modal, it was a prompt with a mode indicator.
        // Motions and unbound keys only -- `i`, `a`, `o` and friends are commands that ask for
        // insert mode, and they are supposed to work.
        let (editor, modal) = keyed("hlwb0$zqfm");
        assert!(editor.is_blank(), "it took text: {:?}", editor.text());
        assert_eq!(modal.mode, Mode::Normal, "and it never left normal mode");
    }

    #[test]
    fn i_is_the_way_in() {
        let (editor, modal) = keyed("ihello");
        assert_eq!(modal.mode, Mode::Insert);
        assert_eq!(editor.text(), "hello", "and the `i` itself is not text");
    }

    #[test]
    fn a_appends_after_the_cursor() {
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        editor.insert_str("ab");
        editor.home();
        for c in "aX".chars() {
            handle(
                press(KeyCode::Char(c)),
                &mut editor,
                &mut None,
                false,
                &mut modal,
            );
        }
        assert_eq!(editor.text(), "aXb");
    }

    #[test]
    fn x_deletes_under_the_cursor_and_dd_takes_the_line() {
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        editor.insert_str("abc");
        editor.home();
        handle(
            press(KeyCode::Char('x')),
            &mut editor,
            &mut None,
            false,
            &mut modal,
        );
        assert_eq!(editor.text(), "bc");
        for _ in 0..2 {
            handle(
                press(KeyCode::Char('d')),
                &mut editor,
                &mut None,
                false,
                &mut modal,
            );
        }
        assert!(editor.is_blank(), "dd left {:?}", editor.text());
    }

    #[test]
    fn a_colon_opens_the_command_line_in_insert_mode() {
        // `:` is a normal-mode command whose whole job is to start typing one.
        let (editor, modal) = keyed(":");
        assert_eq!(editor.text(), ":");
        assert_eq!(modal.mode, Mode::Insert, "so the rest can be typed");
    }

    #[test]
    fn the_arrows_move_the_cursor_in_both_modes() {
        // A modal prompt that will not take an arrow key is being difficult for its own sake.
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        editor.insert_str("abc");
        handle(
            press(KeyCode::Left),
            &mut editor,
            &mut None,
            false,
            &mut modal,
        );
        assert_eq!(editor.cursor().1, 2);
        assert_eq!(modal.mode, Mode::Normal, "and it stays in normal mode");
    }

    #[test]
    fn j_and_k_scroll_the_transcript_when_the_prompt_is_one_line() {
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        assert_eq!(
            handle(
                press(KeyCode::Char('j')),
                &mut editor,
                &mut None,
                false,
                &mut modal
            ),
            Action::Scroll(Scroll::LineDown)
        );
    }

    #[test]
    fn leaving_insert_mode_pulls_the_cursor_back_onto_a_character() {
        // Insert mode rests one past the end of the line and normal mode sits *on* a
        // character. Without this every `x` after an escape deletes one to the left of the
        // block you can see.
        let mut editor = Editor::new();
        let mut modal = typing();
        editor.insert_str("abc");
        assert_eq!(editor.cursor().1, 3, "insert mode is past the end");
        handle(
            press(KeyCode::Esc),
            &mut editor,
            &mut None,
            false,
            &mut modal,
        );
        assert_eq!(editor.cursor().1, 2, "and normal mode is on the `c`");
    }

    #[test]
    fn an_open_menu_still_takes_the_keys() {
        // Normal mode is about the prompt. While a list is open the keys are the list's, or
        // typing to narrow one would be a stream of unbound commands.
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        let mut overlay = axon_tui::complete::resolve(":", 1, &|_| Vec::new()).map(Into::into);
        assert!(overlay.is_some(), "the premise");
        handle(
            press(KeyCode::Char('h')),
            &mut editor,
            &mut overlay,
            false,
            &mut modal,
        );
        assert_eq!(modal.mode, Mode::Normal, "the menu handled it, not vim");
    }
}
