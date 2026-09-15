//! The key table, exercised.
//!
//! Split out under THE RULE; the handler these drive is next door.

use super::*;
use magi_tui::complete;
use magi_tui::vim::Mode;

pub(super) fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

pub(super) fn no_paths(_: &str) -> Vec<String> {
    Vec::new()
}

fn act(key: KeyEvent, editor: &mut Editor, busy: bool) -> Action {
    handle(key, editor, &mut None, &mut None, 20, busy, &mut typing())
}

/// A prompt already in insert mode, which is what everything below the modal tests is about.
pub(super) fn typing() -> Modal {
    let mut modal = Modal::default();
    modal.insert();
    modal
}

/// An editor holding `text`, with the completion popup its content would open.
pub(super) fn with_popup(text: &str) -> (Editor, Option<magi_tui::overlay::Overlay>) {
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
            &mut None,
            20,
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
            &mut None,
            20,
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
            &mut None,
            20,
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
        &mut None,
        20,
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
    let mut editor = Editor::new();
    let mut modal = Modal::default();
    editor.insert_str("a half written prompt");
    modal.open_command(&mut editor);
    editor.insert_str("qu");
    let mut popup = magi_tui::complete::resolve(":qu", 3, &no_paths).map(Into::into);
    let action = handle(
        press(KeyCode::Enter, KeyModifiers::NONE),
        &mut editor,
        &mut popup,
        &mut None,
        20,
        false,
        &mut modal,
    );
    assert_eq!(action, Action::Command(":quit".into()));
    assert_eq!(
        editor.text(),
        "a half written prompt",
        "and the prompt it was holding came back"
    );
}

#[test]
fn enter_on_a_path_completion_only_completes() {
    // A path is part of a sentence, not the whole of one: the line it belongs to is not
    // finished, so submitting it would send half a thought.
    let mut editor = Editor::new();
    editor.insert_str("look at @Car");
    let (_, col) = editor.cursor();
    let line = editor.lines()[0].clone();
    let mut popup: Option<magi_tui::overlay::Overlay> =
        complete::resolve(&line, col, &no_paths).map(Into::into);
    if popup.is_none() {
        return;
    }
    let action = handle(
        press(KeyCode::Enter, KeyModifiers::NONE),
        &mut editor,
        &mut popup,
        &mut None,
        20,
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
        &mut None,
        20,
        false,
        &mut typing(),
    );
    assert_eq!(
        popup
            .as_mut()
            .and_then(magi_tui::overlay::Overlay::completion)
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
        &mut None,
        20,
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
        magi_tui::picker::Picker::new(
            "read wants to read /etc/hosts",
            vec![magi_tui::picker::Choice {
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
        &mut None,
        20,
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
        &mut None,
        20,
        false,
        &mut typing(),
    );
    assert!(popup.is_none());
    assert_eq!(editor.text(), ":qu", "the buffer is untouched");
}
