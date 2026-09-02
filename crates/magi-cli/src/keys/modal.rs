//! Normal mode, and the command line.
//!
//! The prompt opens in normal mode and stays there until something asks for insert mode. Every
//! key is a command, and the one thing none of them do is put themselves in the buffer — which
//! is the whole of what modal means.
//!
//! Split from [`super`] because the two halves answer different questions: that file decides
//! who owns a keystroke — an open list, the prompt, the session — and this one decides what a
//! key means once the prompt has it and is not taking text.

use super::{Action, Scroll};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use magi_tui::Editor;
use magi_tui::vim::{self, Mode, Operator, Wants};

/// What normal mode is in the middle of.
///
/// A half-typed command, held between keystrokes. Kept as one field rather than several flags
/// because they are mutually exclusive by construction — you cannot be waiting for the second
/// `g` of `gg` and for the character `f` wants at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Half {
    /// `g` has been pressed.
    G,
    /// An operator has, and is waiting for a motion.
    Operator(Operator),
    /// `f`, `t`, `F`, `T` or `r` has, and is waiting for a character.
    Wants(Wants),
    /// An operator *and* a find, as in `dt,`.
    OperatorWants(Operator, Wants),
}

/// The prompt's mode, whatever command is half-typed, and whatever text is being held for you.
#[derive(Debug, Default)]
pub struct Modal {
    /// Which mode the prompt is in.
    pub mode: Mode,
    /// A command that has been started and is waiting for another key.
    half: Option<Half>,
    /// The prompt's text, put aside while the command line is open.
    ///
    /// `:` empties the box so the command line has it to itself, and gives it back when the
    /// command runs or is abandoned. Without this, opening a command line over a half-written
    /// prompt means choosing between losing the prompt and typing the command into the middle
    /// of it.
    stashed: Option<String>,
}

impl Modal {
    /// Go to insert mode.
    pub fn insert(&mut self) {
        self.mode = Mode::Insert;
        self.half = None;
    }

    /// Go to normal mode, pulling the cursor back onto a character.
    ///
    /// Insert mode legitimately rests one past the end of the line and normal mode sits *on* a
    /// character, so leaving without this puts the block cursor in a column that has nothing
    /// in it — and every subsequent `x` deletes one character to the left of where it looks.
    pub fn normal(&mut self, editor: &mut Editor) {
        self.mode = Mode::Normal;
        self.half = None;
        editor.settle();
    }

    /// Open the command line, putting the prompt's text aside.
    pub fn open_command(&mut self, editor: &mut Editor) {
        self.stashed = Some(editor.text());
        editor.clear();
        editor.insert(':');
        self.mode = Mode::Command;
        self.half = None;
    }

    /// Close the command line and give the prompt its text back.
    ///
    /// Always to normal mode: what comes back is text you were done typing, and dropping into
    /// insert on top of it would put the cursor in the middle of a sentence you did not ask to
    /// edit. `i` is one key.
    pub fn close_command(&mut self, editor: &mut Editor) {
        if let Some(text) = self.stashed.take() {
            editor.clear();
            editor.insert_str(&text);
        }
        self.mode = Mode::Normal;
        self.half = None;
        editor.settle();
    }

    /// Whether the command line is open.
    #[must_use]
    pub fn commanding(&self) -> bool {
        self.mode == Mode::Command
    }
}

/// Run whatever is on the command line, and give the prompt its text back either way.
pub(super) fn finish_command(editor: &mut Editor, modal: &mut Modal) -> Action {
    let typed = editor.text();
    modal.close_command(editor);
    match typed.strip_prefix(':') {
        // A bare `:` is somebody who changed their mind, which is not an unknown command.
        Some(rest) if rest.trim().is_empty() => Action::Redraw,
        Some(_) => Action::Command(typed),
        None => Action::Redraw,
    }
}

/// One key in normal mode.
///
/// Every path out of here either does something or does nothing. What it never does is put the
/// key in the buffer: that is what insert mode is for, and reaching it is a command like any
/// other.
pub(super) fn normal(key: KeyEvent, editor: &mut Editor, modal: &mut Modal, busy: bool) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let half = modal.half.take();

    if let Some(action) = shared(key, editor, busy, ctrl) {
        return action;
    }

    let KeyCode::Char(c) = key.code else {
        return Action::Ignore;
    };
    if ctrl || key.modifiers.contains(KeyModifiers::ALT) {
        return Action::Ignore;
    }
    let tall = editor.height() > 1;

    // A half-typed command decides what this key is before the key does. `d` then `w` is a
    // motion under an operator, not the `w` binding; `f` then `d` is a character to find, not
    // a delete.
    match half {
        Some(Half::G) => return act(vim::after_g(c, tall), editor, modal, tall),
        Some(Half::Wants(wants)) => return act(wants.given(c), editor, modal, tall),
        Some(Half::OperatorWants(operator, wants)) => {
            return operate(operator, wants.given(c), editor, modal, tall);
        }
        Some(Half::Operator(operator)) => return under(operator, c, editor, modal, tall),
        None => {}
    }

    if c == 'g' {
        modal.half = Some(Half::G);
        return Action::Redraw;
    }
    act(vim::deed(c, tall), editor, modal, tall)
}

/// A key pressed under a waiting operator.
fn under(
    operator: Operator,
    key: char,
    editor: &mut Editor,
    modal: &mut Modal,
    tall: bool,
) -> Action {
    // `dd`, `cc`, `yy`: the operator doubled takes the whole line.
    if vim::doubled(operator, key) {
        return whole_line(operator, editor, modal);
    }
    match vim::deed(key, tall) {
        // `dt,` and friends: the operator is still waiting, and now so is the find.
        vim::Deed::Await(wants) => {
            modal.half = Some(Half::OperatorWants(operator, wants));
            Action::Redraw
        }
        deed @ vim::Deed::Move(_) => operate(operator, deed, editor, modal, tall),
        // `dg` is only ever `dgg`, which this does not have. Anything else under an operator is
        // not a motion, and an operator given a non-motion in vim does nothing at all.
        _ => Action::Ignore,
    }
}

/// Run an operator over the ground a motion covers.
fn operate(
    operator: Operator,
    deed: vim::Deed,
    editor: &mut Editor,
    modal: &mut Modal,
    _tall: bool,
) -> Action {
    let vim::Deed::Move(motion) = deed else {
        return Action::Ignore;
    };
    let from = editor.cursor();
    if !vim::travel(motion, editor) {
        // The motion found nowhere to go, so there is no ground to cover. Deleting to wherever
        // the cursor happened to stop is the wrong answer to `df,` on a line with no comma.
        editor.goto(from.0, from.1);
        return Action::Ignore;
    }
    let mut to = editor.cursor();
    // Inclusive motions take the character they land on; exclusive ones stop before it. `dw`
    // leaves the word it stops at and `de` eats the one it is in, and that difference is not a
    // detail — it is why both keys exist. Forward `f` and `t` are inclusive for the same
    // reason; backwards, `F` and `T` are not.
    let inclusive = matches!(
        motion,
        vim::Motion::WordEnd | vim::Motion::ToChar { forward: true, .. }
    );
    if inclusive {
        to.1 += 1;
    }
    match operator {
        Operator::Yank => {
            editor.copy(from, to);
            editor.goto(from.0.min(to.0), if from <= to { from.1 } else { to.1 });
        }
        Operator::Delete => {
            editor.remember();
            editor.cut(from, to);
        }
        Operator::Change => {
            editor.remember();
            editor.cut(from, to);
            modal.insert();
        }
    }
    editor.settle();
    Action::Redraw
}

/// `dd`, `cc`, `yy`.
fn whole_line(operator: Operator, editor: &mut Editor, modal: &mut Modal) -> Action {
    let (row, _) = editor.cursor();
    match operator {
        Operator::Yank => editor.copy_lines(row, row),
        Operator::Delete => {
            editor.remember();
            editor.delete_line();
        }
        Operator::Change => {
            editor.remember();
            editor.copy_lines(row, row);
            editor.kill_to_start();
            editor.kill_to_end();
            modal.insert();
        }
    }
    editor.settle();
    Action::Redraw
}

/// Carry out a resolved key.
fn act(deed: vim::Deed, editor: &mut Editor, modal: &mut Modal, _tall: bool) -> Action {
    match deed {
        vim::Deed::Move(motion) => {
            vim::travel(motion, editor);
            editor.settle();
            Action::Redraw
        }
        vim::Deed::Edit(edit) => {
            if vim::changes(edit) {
                editor.remember();
            }
            vim::apply(edit, editor);
            // Back onto a character, because every edit here can leave the cursor past the end
            // of a line it has just shortened.
            editor.settle();
            Action::Redraw
        }
        vim::Deed::Operate(operator) => {
            modal.half = Some(Half::Operator(operator));
            Action::Redraw
        }
        vim::Deed::Await(wants) => {
            modal.half = Some(Half::Wants(wants));
            Action::Redraw
        }
        vim::Deed::Insert(first) => {
            if let Some(edit) = first {
                if vim::changes(edit) {
                    editor.remember();
                }
                vim::apply(edit, editor);
            }
            modal.insert();
            Action::Redraw
        }
        vim::Deed::Command => {
            modal.open_command(editor);
            Action::Redraw
        }
        vim::Deed::Undo => {
            if editor.undo() {
                editor.settle();
            }
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
        vim::Deed::Submit => submit(editor),
        vim::Deed::Unbound => Action::Ignore,
    }
}

/// Send what is in the buffer.
fn submit(editor: &mut Editor) -> Action {
    match editor.submit() {
        Some(text) if text.starts_with(':') => Action::Command(text),
        Some(text) => Action::Submit(text),
        None => Action::Ignore,
    }
}

/// The keys that mean the same thing in normal mode as anywhere else.
///
/// Arrows especially: a modal prompt that will not take an arrow key is being difficult for the
/// sake of it. `None` means normal mode has to work the key out for itself.
fn shared(key: KeyEvent, editor: &mut Editor, busy: bool, ctrl: bool) -> Option<Action> {
    let tall = editor.height() > 1;
    Some(match key.code {
        KeyCode::Esc if busy => Action::Interrupt,
        KeyCode::Esc => Action::Redraw,
        KeyCode::Left => {
            editor.left();
            Action::Redraw
        }
        KeyCode::Right => {
            editor.right();
            Action::Redraw
        }
        KeyCode::Up if tall => {
            editor.up();
            Action::Redraw
        }
        KeyCode::Down if tall => {
            editor.down();
            Action::Redraw
        }
        KeyCode::Up => Action::Scroll(Scroll::LineUp),
        KeyCode::Down => Action::Scroll(Scroll::LineDown),
        KeyCode::Home => {
            editor.home();
            Action::Redraw
        }
        KeyCode::End => {
            editor.end();
            Action::Redraw
        }
        KeyCode::PageUp => Action::Scroll(Scroll::PageUp),
        KeyCode::PageDown => Action::Scroll(Scroll::PageDown),
        KeyCode::Char('c') if ctrl => {
            editor.clear();
            Action::Redraw
        }
        KeyCode::Char('x') if ctrl => Action::ExternalEdit,
        KeyCode::Char('o') if ctrl => Action::ToggleDetail,
        KeyCode::Char('d') if ctrl => Action::Scroll(Scroll::PageDown),
        KeyCode::Char('u') if ctrl => Action::Scroll(Scroll::PageUp),
        KeyCode::Char('r') if ctrl => {
            // Redo would go here. It is not `u` backwards -- it needs its own stack -- and an
            // unbound key is better than one that does something almost right.
            Action::Ignore
        }
        KeyCode::Enter if busy => Action::Ignore,
        KeyCode::Enter => submit(editor),
        _ => return None,
    })
}
/// The prompt is modal: a letter is a command until you say otherwise.
#[cfg(test)]
mod modal_tests {
    use super::super::handle;
    use magi_tui::vim::Mode;

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
    fn a_colon_opens_a_command_line_and_holds_the_prompt() {
        // `:` is a normal-mode command whose job is to start typing one, and the text you had
        // is put aside rather than typed into.
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        editor.insert_str("a half written prompt");
        handle(
            press(KeyCode::Char(':')),
            &mut editor,
            &mut None,
            false,
            &mut modal,
        );
        assert_eq!(modal.mode, Mode::Command);
        assert_eq!(editor.text(), ":", "the box is the command line now");
        modal.close_command(&mut editor);
        assert_eq!(
            editor.text(),
            "a half written prompt",
            "and the prompt came back"
        );
        assert_eq!(
            modal.mode,
            Mode::Normal,
            "in normal mode, so `i` is one key"
        );
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
        let mut overlay = magi_tui::complete::resolve(":", 1, &|_| Vec::new()).map(Into::into);
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

/// Operators, motions, and the two composing.
#[cfg(test)]
mod motion_tests {
    use super::super::handle;
    use super::*;

    /// A prompt holding `text` with the cursor at the start, after `keys` in normal mode.
    fn after(text: &str, keys: &str) -> Editor {
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        editor.insert_str(text);
        editor.goto(0, 0);
        for c in keys.chars() {
            handle(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut editor,
                &mut None,
                false,
                &mut modal,
            );
        }
        editor
    }

    #[test]
    fn dw_deletes_a_word() {
        assert_eq!(after("one two three", "dw").text(), "two three");
    }

    #[test]
    fn d_dollar_deletes_to_the_end_of_the_line() {
        assert_eq!(after("one two", "ld$").text(), "o");
    }

    #[test]
    fn cw_deletes_a_word_and_leaves_you_typing() {
        let mut editor = Editor::new();
        let mut modal = Modal::default();
        editor.insert_str("one two");
        editor.goto(0, 0);
        for c in "cw".chars() {
            handle(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                &mut editor,
                &mut None,
                false,
                &mut modal,
            );
        }
        assert_eq!(editor.text(), "two");
        assert_eq!(modal.mode, Mode::Insert, "change ends in insert mode");
    }

    #[test]
    fn yy_then_p_puts_the_line_below() {
        // The linewise register, which is the difference between `yy p` and `yw p`.
        assert_eq!(after("one\ntwo", "yyp").text(), "one\none\ntwo");
    }

    #[test]
    fn yw_then_p_puts_the_word_after_the_cursor() {
        assert_eq!(after("ab cd", "ywp").text(), "aab b cd");
    }

    #[test]
    fn f_finds_a_character_and_df_takes_it_with_it() {
        assert_eq!(after("a,b,c", "f,").cursor().1, 1);
        assert_eq!(after("a,b,c", "df,").text(), "b,c");
    }

    #[test]
    fn t_stops_short_of_it() {
        assert_eq!(after("a,b", "t,").cursor().1, 0);
        assert_eq!(after("abc,d", "dt,").text(), ",d");
    }

    #[test]
    fn a_find_with_nothing_to_find_changes_nothing() {
        // `df,` on a line with no comma must leave the line alone rather than deleting to
        // wherever the cursor happened to stop.
        assert_eq!(after("no commas", "df,").text(), "no commas");
    }

    #[test]
    fn u_walks_back_one_command_at_a_time() {
        // One command is one undo. `dw` is three editor calls and must not take three `u`.
        assert_eq!(after("one two three", "dwdw").text(), "three");
        assert_eq!(after("one two three", "dwdwu").text(), "two three");
        assert_eq!(after("one two three", "dwdwuu").text(), "one two three");
    }

    #[test]
    fn moving_about_is_not_worth_an_undo() {
        // Otherwise `u` after a walk across the line undoes the walk, which looks like `u`
        // doing nothing at all.
        assert_eq!(after("one two", "dwwwbbu").text(), "one two");
    }

    #[test]
    fn r_replaces_one_character() {
        assert_eq!(after("cat", "rb").text(), "bat");
    }

    #[test]
    fn x_and_shift_x_take_from_either_side() {
        assert_eq!(after("abc", "x").text(), "bc");
        assert_eq!(after("abc", "llX").text(), "ac");
    }

    #[test]
    fn shift_j_joins_the_next_line_on() {
        assert_eq!(after("one\ntwo", "J").text(), "one two");
    }

    #[test]
    fn tilde_flips_the_case_and_moves_on() {
        assert_eq!(after("abc", "~~").text(), "ABc");
    }

    #[test]
    fn an_operator_given_something_that_is_not_a_motion_does_nothing() {
        // vim's behaviour, and the safe one: `dz` is a typo, and a typo should not eat a line.
        assert_eq!(after("one two", "dz").text(), "one two");
    }

    #[test]
    fn e_goes_to_the_end_of_the_word() {
        assert_eq!(after("one two", "e").cursor().1, 2);
        assert_eq!(after("one two", "de").text(), " two");
    }
}
