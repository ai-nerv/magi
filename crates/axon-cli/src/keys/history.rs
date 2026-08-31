//! Reaching earlier prompts past a slash command.
//!
//! The complaint this answers: "once i hit /<command> then i cant go up further". A line
//! beginning with `/` opens the command menu, the menu owns the arrows, and history stopped
//! dead at the first slash command in it — including one it had just recalled.

use super::tests::{no_paths, press, with_popup};
use super::*;
use axon_tui::complete;

/// Which row of an open completion popup is highlighted.
fn highlight(open: &mut Option<axon_tui::overlay::Overlay>) -> usize {
    open.as_mut()
        .and_then(axon_tui::overlay::Overlay::completion)
        .expect("still open")
        .selected
}

fn remembered() -> Editor {
    Editor::with_history(vec![
        "the oldest thing".to_owned(),
        "/model".to_owned(),
        "the newest thing".to_owned(),
    ])
}

#[test]
fn up_walks_back_through_earlier_prompts() {
    let mut editor = remembered();
    let mut none = None;
    assert_eq!(
        handle(
            press(KeyCode::Up, KeyModifiers::NONE),
            &mut editor,
            &mut none,
            false
        ),
        Action::Recalled
    );
    assert_eq!(editor.lines()[0], "the newest thing");
}

#[test]
fn a_recalled_line_does_not_reopen_the_menu_over_itself() {
    // `Recalled` rather than `Redraw` is the whole fix: the driver rebuilds the popup from
    // the prompt after every key *except* these, so recalling `/model` leaves the menu shut
    // and the next Up is still history's.
    let mut editor = remembered();
    let mut none = None;
    let mut seen = Vec::new();
    for _ in 0..3 {
        let action = handle(
            press(KeyCode::Up, KeyModifiers::NONE),
            &mut editor,
            &mut none,
            false,
        );
        assert_eq!(action, Action::Recalled, "the popup must not be rebuilt");
        seen.push(editor.lines()[0].clone());
    }
    assert_eq!(
        seen,
        vec!["the newest thing", "/model", "the oldest thing"],
        "history walks straight past the slash command"
    );
}

#[test]
fn the_menu_still_owns_the_arrows_while_there_is_menu_left() {
    let (mut editor, mut popup) = with_popup("/");
    assert!(popup.is_some(), "the premise: `/` opens the menu");
    let action = handle(
        press(KeyCode::Down, KeyModifiers::NONE),
        &mut editor,
        &mut popup,
        false,
    );
    assert_eq!(action, Action::Redraw);
    assert_eq!(
        highlight(&mut popup),
        1,
        "Down moved the highlight, not history"
    );
    // And Up from there walks back up the menu rather than leaving it.
    handle(
        press(KeyCode::Up, KeyModifiers::NONE),
        &mut editor,
        &mut popup,
        false,
    );
    assert_eq!(highlight(&mut popup), 0);
}

#[test]
fn up_at_the_top_of_the_menu_leaves_it_for_history() {
    // It used to wrap to the bottom, which made the menu a wall: typing `/` put it between
    // the user and every earlier prompt, with no key that got past it.
    let mut editor = Editor::with_history(vec!["an earlier prompt".to_owned()]);
    editor.insert_str("/");
    let (_, col) = editor.cursor();
    let line = editor.lines()[0].clone();
    let mut popup: Option<axon_tui::overlay::Overlay> =
        complete::resolve(&line, col, &no_paths).map(Into::into);
    assert_eq!(highlight(&mut popup), 0, "starts at the top");

    let action = handle(
        press(KeyCode::Up, KeyModifiers::NONE),
        &mut editor,
        &mut popup,
        false,
    );
    assert_eq!(action, Action::Recalled);
    assert!(popup.is_none(), "the menu got out of the way");
    assert_eq!(editor.lines()[0], "an earlier prompt");
}

/// Handing the mouse back, so the terminal can do what a terminal does.
#[cfg(test)]
mod the_mouse_is_the_terminals {
    use super::super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn ctrl_t_is_not_a_binding() {
        // There was one, for taking the mouse. Nothing takes the mouse now -- an application
        // that turns on tracking stops the terminal selecting text everywhere -- so the mode it
        // switched between no longer exists, and a key for it was a key to get back what should
        // never have been taken.
        let mut editor = Editor::new();
        let action = handle(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
            &mut editor,
            &mut None,
            false,
        );
        assert_eq!(action, Action::Ignore);
    }

    #[test]
    fn a_plain_t_is_still_a_letter() {
        let mut editor = Editor::new();
        handle(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            &mut editor,
            &mut None,
            false,
        );
        assert_eq!(editor.lines()[0], "t");
    }
}
