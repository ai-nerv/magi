//! Walking an open completion list, and the rebuild that used to undo it: the driver rebuilt the
//! popup from the prompt on the next line of the loop, and a rebuilt popup starts at the top. The
//! tests here assert the *action* rather than the highlight, because the reset happened above.

use super::tests::{no_paths, press, with_popup};
use super::*;

/// Which row of an open completion popup is highlighted.
fn highlight(open: &mut Option<magi_tui::overlay::Overlay>) -> usize {
    open.as_mut()
        .and_then(magi_tui::overlay::Overlay::completion)
        .expect("still open")
        .selected
}

/// Whether the driver would rebuild the popup after this action — the driver's own predicate, not
/// a copy of it.
use super::recomputes as rebuilds;

#[test]
fn moving_the_highlight_does_not_ask_for_the_list_to_be_rebuilt() {
    // Down returned `Redraw`, which is also what typing returns, so the driver could not tell a
    // changed prompt from a moved highlight and recomputed either way.
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
    // Three presses reach the third row; before the fix each one moved to row 1 and was reset.
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
    // The guard cannot simply be "a completion is open": typing is how you narrow the menu.
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
    // A menu that wraps is one you cannot walk out of.
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
