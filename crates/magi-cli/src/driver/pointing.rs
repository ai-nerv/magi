//! What the pointer does, and who it belongs to.
//!
//! Two readers, and the order between them is the whole of this file: a surface owns the pointer
//! over its own rows, and everything else on the screen is magi's — the wheel, the fold handles,
//! the copy chips, and the drag that selects text.
//!
//! **magi asks for the pointer.** Mouse reporting is one terminal-wide switch, so an application
//! that turns it on to receive a click stops the terminal running its own drag-selection. That is
//! why magi does the selecting itself — see [`magi_tui::select`] — and it is why a surface can be
//! clicked at all.

use crate::app::App;
use crossterm::event::{MouseEvent, MouseEventKind};
use magi_proto::UiCommand;

/// What the loop should do with the frame after a pointer event.
pub(crate) enum Pointing {
    /// Something changed on screen.
    Redraw,
    /// Nothing did. Most of them: the pointer crosses a cell that lights nothing up.
    Nothing,
}

/// Hand the pointer to a surface, if it landed on the rows one is holding.
///
/// **Translated on the way through and never interpreted.** magi turns a screen cell into one of
/// the tenant's, which is the one thing it knows and the tenant cannot; what a click at row two
/// means is the tenant's business. Anything outside the reservation is not forwarded at all, so
/// the transcript keeps its wheel and its handles while a game is open below it.
pub(crate) async fn to_surface(
    app: &App,
    mouse: MouseEvent,
    commands: &tokio::sync::mpsc::Sender<UiCommand>,
) -> bool {
    let Some(held) = app.holding() else {
        return false;
    };
    let (Some((row, col)), Some((kind, button))) = (
        app.pointed_at(mouse.row, mouse.column),
        crate::keying::pointed(mouse.kind),
    ) else {
        return false;
    };
    let _ = commands
        .send(UiCommand::Moused {
            id: held.id.clone(),
            kind,
            button,
            row,
            col,
        })
        .await;
    true
}

/// The pointer over magi's own screen: the transcript, its handles and its chips.
///
/// `copied` is set by a release and acted on after the next draw, because the text a selection
/// covers is read out of the frame that drew it.
pub(crate) fn on_the_screen(
    app: &mut App,
    mouse: MouseEvent,
    view: u16,
    width: u16,
    copied: &mut Option<magi_tui::select::Selection>,
) -> Pointing {
    use crossterm::event::MouseButton;
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scrollback.scroll_up(3),
        MouseEventKind::ScrollDown => app.scrollback.scroll_down(3, view),
        // The pointer passing over a fold handle lights it up. Every cell it crosses arrives here
        // and all but a handful change nothing — `hover_at` says which, and only those cost a
        // frame.
        MouseEventKind::Moved => {
            if !app.hover_at(mouse.row, mouse.column) {
                return Pointing::Nothing;
            }
        }
        // A tool block opens and closes under the pointer. Ctrl+O still moves the whole transcript
        // at once; this is for the one result you actually want to read, which is usually not the
        // newest. The handle first: it is the one thing on screen that is a button, and a press on
        // it is a press on it rather than the start of a one-character selection.
        MouseEventKind::Down(MouseButton::Left) => {
            app.selection = None;
            // **The usage badge is a button.** It is the number you are looking at when you
            // wonder what a session has cost, so a press on it opens the view that answers that
            // -- the shortest path from noticing to knowing. First, because it sits in the
            // prompt box's edge and a press that fell through would start a selection instead.
            if app
                .corner_rect
                .is_some_and(|at| within(at, mouse.row, mouse.column))
            {
                app.press_corner();
                return Pointing::Redraw;
            }
            // Copy first: both chips sit in the same edge, and a press that fell through to the
            // fold would open the block a person meant to take a copy of.
            if let Some(text) = app.copy_at(mouse.row, mouse.column, width) {
                crate::clipboard::put(&text);
            } else if !app.toggle_at(mouse.row, mouse.column, width) {
                app.selection = Some(magi_tui::select::Selection::begin(mouse.row, mouse.column));
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(sel) = app.selection.as_mut() {
                sel.drag_to(mouse.row, mouse.column);
            } else {
                return Pointing::Nothing;
            }
        }
        // Copied on release, because that is when a person has finished choosing. Through OSC 52,
        // which is the clipboard a terminal will accept through a multiplexer and over ssh alike.
        MouseEventKind::Up(MouseButton::Left) => {
            let Some(sel) = app.selection.as_mut() else {
                return Pointing::Nothing;
            };
            sel.drag_to(mouse.row, mouse.column);
            sel.finish();
            if sel.is_empty() {
                app.selection = None;
            } else {
                *copied = app.selection;
            }
        }
        _ => return Pointing::Nothing,
    }
    Pointing::Redraw
}

/// Whether a press landed inside a rectangle.
///
/// Its own function because a click target is an easy thing to get subtly wrong — an off-by-one
/// on the right edge means the last column of a button does nothing, which reads as a button that
/// works sometimes.
fn within(at: ratatui::layout::Rect, row: u16, column: u16) -> bool {
    row >= at.y && row < at.y + at.height && column >= at.x && column < at.x + at.width
}

#[cfg(test)]
mod hitting {
    use super::within;
    use ratatui::layout::Rect;

    #[test]
    fn a_click_target_includes_its_own_edges_and_nothing_past_them() {
        // An off-by-one on the right edge means the last column of a button does nothing, which
        // reads as a button that works sometimes — the worst kind to debug, because the person
        // reporting it was clicking in the right place.
        let at = Rect {
            x: 90,
            y: 28,
            width: 18,
            height: 1,
        };
        assert!(within(at, 28, 90), "left edge");
        assert!(within(at, 28, 107), "right edge");
        assert!(!within(at, 28, 108), "one past it");
        assert!(!within(at, 28, 89), "one before it");
        assert!(!within(at, 27, 95), "the row above");
        assert!(!within(at, 29, 95), "and the row below");
    }

    #[test]
    fn an_empty_target_catches_nothing() {
        // A session that has spent nothing wears no badge, and `usage_rect` is `None` — but a
        // zero-width rect must not swallow clicks either.
        let none = Rect {
            x: 10,
            y: 5,
            width: 0,
            height: 0,
        };
        assert!(!within(none, 5, 10));
    }
}
