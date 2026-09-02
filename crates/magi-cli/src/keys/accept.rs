//! Taking a completion, and what happens next.

use super::tests::typing;
use super::*;
use crossterm::event::KeyModifiers;

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
    let mut overlay: Option<magi_tui::overlay::Overlay> =
        magi_tui::complete::resolve(":hel", 4, &|_| Vec::new()).map(Into::into);
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
