//! Walking an open completion list, and the rebuild that used to undo it.
//!
//! The complaint this answers: the arrow keys did nothing on the command list. Every piece
//! worked — `Completion::next` moved the highlight, the key handler called it, the popup drew
//! the row it was told to — and the feature was still dead, because the driver rebuilt the popup
//! from the prompt on the next line of the loop and a rebuilt popup starts at the top.
//!
//! That is why the test here is about the *action* rather than about the highlight. Asserting
//! `selected` after one keypress passed the whole time this was broken: the reset happened in
//! the driver, one layer above anything the key tests could see.

use super::tests::{no_paths, press, with_popup};
use super::*;

/// Which row of an open completion popup is highlighted.
fn highlight(open: &mut Option<magi_tui::overlay::Overlay>) -> usize {
    open.as_mut()
        .and_then(magi_tui::overlay::Overlay::completion)
        .expect("still open")
        .selected
}

/// Whether the driver would rebuild the popup after this action.
///
/// **The driver's own predicate, not a copy of it.** A test that restated the rule would pass
/// while the driver used a different one, which is exactly the shape of the bug being fixed:
/// two places that had to agree and nothing making them.
use super::recomputes as rebuilds;

#[test]
fn moving_the_highlight_does_not_ask_for_the_list_to_be_rebuilt() {
    // **The bug, stated as the thing that was actually wrong.** Down returned `Redraw`, which is
    // also what typing returns, so the driver could not tell "the prompt changed, recompute the
    // menu" from "the prompt did not change, I just moved" — and recomputed either way.
    let (mut editor, mut overlay) = with_popup(":");
    let down = press(KeyCode::Down, KeyModifiers::NONE);
    let action = handle(
        down,
        &mut editor,
        &mut overlay,
        &mut None,
        20,
        false,
        &mut Modal::default(),
    );

    assert_eq!(action, Action::Moved, "not Redraw, which means recompute");
    assert!(!rebuilds(&action), "and the driver leaves the list alone");
    assert_eq!(highlight(&mut overlay), 1);
}

#[test]
fn walking_down_the_list_keeps_walking() {
    // Three presses reach the third row. Before the fix each one moved to row 1 and was reset,
    // so the list was permanently on its first entry however long you held the key.
    let (mut editor, mut overlay) = with_popup(":");
    for expected in 1..=3 {
        let action = handle(
            press(KeyCode::Down, KeyModifiers::NONE),
            &mut editor,
            &mut overlay,
            &mut None,
            20,
            false,
            &mut Modal::default(),
        );
        assert_eq!(action, Action::Moved);
        assert_eq!(highlight(&mut overlay), expected);
    }
}

#[test]
fn typing_still_rebuilds_the_list() {
    // The other half, and the reason the guard cannot simply be "a completion is open": typing
    // is how you narrow the menu, and a menu that stopped narrowing would be the opposite bug.
    let (mut editor, mut overlay) = with_popup(":");
    let action = handle(
        press(KeyCode::Char('m'), KeyModifiers::NONE),
        &mut editor,
        &mut overlay,
        &mut None,
        20,
        false,
        &mut Modal::default(),
    );
    assert!(
        rebuilds(&action),
        "a keystroke that changed the prompt recomputes the menu: {action:?}"
    );
}

#[test]
fn up_from_the_top_still_leaves_for_history() {
    // Unchanged by the fix, and worth pinning beside it: a menu that wraps is one you cannot
    // walk out of, and typing `:` put it between the person and every earlier prompt.
    let (mut editor, mut overlay) = with_popup(":");
    let action = handle(
        press(KeyCode::Up, KeyModifiers::NONE),
        &mut editor,
        &mut overlay,
        &mut None,
        20,
        false,
        &mut Modal::default(),
    );
    assert_eq!(action, Action::Recalled);
    assert!(overlay.is_none(), "the menu closed rather than wrapping");
    let _ = no_paths("");
}
