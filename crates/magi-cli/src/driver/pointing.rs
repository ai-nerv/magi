//! What the pointer does, and who it belongs to: a surface owns the pointer over its own rows, and
//! everything else on the screen is magi's. Mouse reporting is one terminal-wide switch, so an
//! application that turns it on stops the terminal running its own drag-selection — which is why
//! magi does the selecting itself, see [`magi_tui::select`].

use crate::app::App;
use crossterm::event::{MouseEvent, MouseEventKind};
use magi_proto::UiCommand;

/// What the loop should do with the frame after a pointer event.
pub(crate) enum Pointing {
    Redraw,
    Nothing,
}

/// Hand the pointer to a surface, if it landed on the rows one is holding. The cell is translated
/// into the tenant's and never interpreted; anything outside the reservation is not forwarded.
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

/// The pointer over magi's own screen: the transcript, its handles and its chips. `copied` is set by
/// a release and acted on after the next draw, because the text is read out of the frame that drew it.
pub(crate) fn on_the_screen(
    app: &mut App,
    mouse: MouseEvent,
    view: u16,
    width: u16,
    copied: &mut Option<magi_tui::select::Selection>,
) -> Pointing {
    use crossterm::event::MouseButton;

    // Read before the match because the rect and the pane are two fields of the same struct, and the
    // arms below borrow the pane.
    let page = app.pane_rect.map_or(0, magi_tui::pane::Pane::page_of);

    match mouse.kind {
        // An open float takes the wheel wherever the pointer is: the keyboard already treats it as
        // modal, and a positional rule would have the wheel doing two things a few cells apart.
        MouseEventKind::ScrollUp => match app.pane.as_mut() {
            Some(open) => open.up(3),
            None => app.scrollback.scroll_up(3),
        },
        MouseEventKind::ScrollDown => match app.pane.as_mut() {
            Some(open) => open.down(3, page),
            None => app.scrollback.scroll_down(3, view),
        },
        // Every cell the pointer crosses arrives here; `hover_at` says which change anything.
        MouseEventKind::Moved => {
            if !app.hover_at(mouse.row, mouse.column) {
                return Pointing::Nothing;
            }
        }
        // The handle first: it is the one thing on screen that is a button, and a press on it is a
        // press rather than the start of a one-character selection.
        MouseEventKind::Down(MouseButton::Left) => {
            app.selection = None;
            // The usage badge is a button, and first because it sits in the prompt box's edge where
            // a press that fell through would start a selection instead.
            if app
                .corner_rect
                .is_some_and(|at| within(at, mouse.row, mouse.column))
            {
                app.press_corner();
                return Pointing::Redraw;
            }
            // A float is dismissed by clicking off it — after the corner, because that badge has its
            // own toggle. A press inside the float goes nowhere: it is drawn over the transcript, so
            // a click falling through would select in a conversation nobody can see. Rect and pane
            // both, because a view closed by a key leaves its rect behind until the next frame.
            if let Some(at) = app.pane_rect.filter(|_| app.pane.is_some()) {
                if !within(at, mouse.row, mouse.column) {
                    app.pane = None;
                }
                return Pointing::Redraw;
            }
            // Copy first: both chips sit in the same edge, and a press that fell through to the fold
            // would open the block a person meant to take a copy of.
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
        // Through OSC 52, which is the clipboard a terminal will accept through a multiplexer and
        // over ssh alike.
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

/// Whether a press landed inside a rectangle. An off-by-one on the right edge means the last column
/// of a button does nothing, which reads as a button that works sometimes.
fn within(at: ratatui::layout::Rect, row: u16, column: u16) -> bool {
    row >= at.y && row < at.y + at.height && column >= at.x && column < at.x + at.width
}

#[cfg(test)]
mod hitting {
    use super::within;
    use ratatui::layout::Rect;

    #[test]
    fn a_click_target_includes_its_own_edges_and_nothing_past_them() {
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
        // A session that has spent nothing wears no badge and `usage_rect` is `None`, but a
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

/// Dismissing the float by clicking off it.
#[cfg(test)]
mod dismissing {
    use super::{Pointing, on_the_screen};
    use crate::app::App;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    /// Where a pane lands on an eighty-by-thirty screen.
    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 30,
    };

    /// An app with a pane open, and its rect recorded as a draw would have.
    fn showing() -> App {
        let mut app = App::new();
        app.show_trace();
        app.pane_rect = Some(magi_tui::pane::Pane::area(SCREEN));
        app
    }

    fn press(row: u16, column: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn clicked(app: &mut App, at: MouseEvent) -> Pointing {
        on_the_screen(app, at, SCREEN.height, SCREEN.width, &mut None)
    }

    #[test]
    fn a_press_outside_the_float_closes_it() {
        let mut app = showing();
        assert!(app.pane.is_some());
        let _ = clicked(&mut app, press(0, 0));
        assert!(
            app.pane.is_none(),
            "a press in the corner should dismiss it"
        );
    }

    #[test]
    fn a_press_inside_the_float_leaves_it_open() {
        let mut app = showing();
        let at = app.pane_rect.expect("a drawn pane");
        let _ = clicked(&mut app, press(at.y + 1, at.x + 1));
        assert!(app.pane.is_some());
    }

    #[test]
    fn a_press_inside_the_float_does_not_reach_what_it_covers() {
        // The pane is drawn over the transcript: a click that fell through would begin a selection
        // in a conversation the person cannot see.
        let mut app = showing();
        let at = app.pane_rect.expect("a drawn pane");
        let _ = clicked(&mut app, press(at.y + 2, at.x + 3));
        assert!(
            app.selection.is_none(),
            "a press on the float began a selection underneath it"
        );
    }

    #[test]
    fn the_edges_belong_to_the_float() {
        // Off-by-one on a dismissal closes the thing you were trying to click on.
        let mut app = showing();
        let at = app.pane_rect.expect("a drawn pane");
        let _ = clicked(&mut app, press(at.y, at.x));
        assert!(app.pane.is_some(), "the top-left corner is the float's");

        let mut app = showing();
        let _ = clicked(&mut app, press(at.y + at.height - 1, at.x + at.width - 1));
        assert!(app.pane.is_some(), "and so is the bottom-right");
    }

    #[test]
    fn a_stale_rect_with_no_pane_open_swallows_nothing() {
        // The rect is a drawing artefact and the pane is the truth: a view closed by a key leaves its
        // rect behind until the next frame, and a click then must reach the transcript.
        let mut app = showing();
        app.pane = None;
        let at = app.pane_rect.expect("the rect the last frame left");
        let _ = clicked(&mut app, press(at.y + 1, at.x + 1));
        assert!(
            app.selection.is_some(),
            "a closed float went on taking clicks"
        );
    }
}

/// Who the wheel belongs to while a float is open.
#[cfg(test)]
mod wheeling {
    use super::on_the_screen;
    use crate::app::App;
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    /// Where a pane lands on an eighty-by-thirty screen.
    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 30,
    };

    /// An app with a long transcript, and a float over it holding more rows than fit.
    fn showing() -> App {
        let mut app = App::new();
        app.scrollback
            .set_lines((0..500).map(|n| Line::from(format!("line {n}"))).collect());
        app.pane = Some(magi_tui::pane::Pane::new(
            "test",
            (0..500).map(|n| Line::from(format!("row {n}"))).collect(),
        ));
        app.pane_rect = Some(magi_tui::pane::Pane::area(SCREEN));
        app
    }

    fn wheel(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn turned(app: &mut App, kind: MouseEventKind) {
        let _ = on_the_screen(app, wheel(kind), SCREEN.height, SCREEN.width, &mut None);
    }

    #[test]
    fn the_wheel_scrolls_the_float_rather_than_the_transcript() {
        // The float is drawn over the transcript, so a wheel moving the transcript would scroll the
        // one thing on screen the person cannot see.
        let mut app = showing();
        turned(&mut app, MouseEventKind::ScrollDown);
        assert_eq!(app.pane.as_ref().expect("a pane").top, 3);
        assert!(
            app.scrollback.is_following(),
            "the transcript moved under the float"
        );
    }

    #[test]
    fn the_wheel_goes_back_up_again() {
        let mut app = showing();
        turned(&mut app, MouseEventKind::ScrollDown);
        turned(&mut app, MouseEventKind::ScrollDown);
        assert_eq!(app.pane.as_ref().expect("a pane").top, 6);
        turned(&mut app, MouseEventKind::ScrollUp);
        assert_eq!(app.pane.as_ref().expect("a pane").top, 3);
    }

    #[test]
    fn the_pointer_being_off_the_float_does_not_give_the_wheel_back() {
        // The panel covers the middle of the screen, so a positional rule would have the wheel doing
        // two different things a few cells apart.
        let mut app = showing();
        let far = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 79,
            row: 29,
            modifiers: KeyModifiers::NONE,
        };
        let _ = on_the_screen(&mut app, far, SCREEN.height, SCREEN.width, &mut None);
        assert_eq!(app.pane.as_ref().expect("a pane").top, 3);
        assert!(app.scrollback.is_following());
    }

    #[test]
    fn with_no_float_open_the_wheel_is_the_transcripts_again() {
        let mut app = showing();
        app.pane = None;
        turned(&mut app, MouseEventKind::ScrollUp);
        assert!(
            !app.scrollback.is_following(),
            "the transcript should have scrolled away from the newest output"
        );
    }

    #[test]
    fn scrolling_stops_at_the_last_page_rather_than_past_it() {
        // Past the end is how a view ends up showing an empty box: the content is above the viewport
        // and there is nothing on screen to say so.
        let mut app = showing();
        for _ in 0..500 {
            turned(&mut app, MouseEventKind::ScrollDown);
        }
        let page = magi_tui::pane::Pane::page_of(app.pane_rect.expect("a drawn pane"));
        assert_eq!(app.pane.as_ref().expect("a pane").top, 500 - page);
    }
}
