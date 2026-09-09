//! Rows a tool is holding, and what it last drew in them. The UI reserves the space, keeps the
//! most recent frame and sends keys to whoever holds it; it never reads what is in there.

use magi_proto::ToolCallId;
use magi_proto::tooling::Span;

/// A surface currently on the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Surfacing {
    pub id: ToolCallId,
    pub tool: String,
    /// The reservation the layout is built from, not the height of the last frame, so the
    /// transcript above does not jump when a tenant draws a shorter one.
    pub rows: u16,
    /// What it is for, shown until it draws its first frame.
    pub about: String,
    pub drawn: Vec<Vec<Span>>,
    /// Where it asked for the terminal's own cursor, in its own coordinates. `None` unless the
    /// tenant draws a field somebody types into.
    pub cursor: Option<magi_proto::surfacing::At>,
}

impl super::App {
    /// Give a tool the rows it asked for.
    pub(super) fn surfaced(&mut self, id: ToolCallId, tool: String, rows: u16, about: String) {
        self.surface = Some(Surfacing {
            id,
            tool,
            rows,
            about,
            drawn: Vec::new(),
            cursor: None,
        });
    }

    /// Keep what a surface drew, if it is the one holding the rows. A frame for a surface that is
    /// not on screen is dropped, or one tool's output lands inside another's.
    pub(super) fn drew(
        &mut self,
        id: &ToolCallId,
        lines: Vec<Vec<Span>>,
        cursor: Option<magi_proto::surfacing::At>,
    ) {
        if let Some(surface) = self.surface.as_mut()
            && surface.id == *id
        {
            surface.drawn = lines;
            // Per frame, so a tenant can take the caret back.
            surface.cursor = cursor;
        }
    }

    /// Take the rows back.
    pub(super) fn unsurfaced(&mut self, id: &ToolCallId) {
        if self.surface.as_ref().is_some_and(|held| held.id == *id) {
            self.surface = None;
        }
    }

    /// Whether something is stopped until the person answers a question on screen: a permission, a
    /// tool's own question and an adoption each hold a caller, and everything else is a convenience.
    #[must_use]
    pub fn questioned(&self) -> bool {
        self.picking.as_ref().is_some_and(super::Picking::blocking)
    }

    /// The surface holding the rows, and nothing while [`Self::questioned`] — a surface owns the
    /// menu slot and the keyboard, which would leave the blocking question unreachable.
    #[must_use]
    pub fn holding(&self) -> Option<&Surfacing> {
        self.surface.as_ref().filter(|_| !self.questioned())
    }

    /// Turn a screen cell into one of the tenant's own, when it landed on its rows. `None`
    /// anywhere else: a surface hears about the pointer over its rows and nothing more.
    #[must_use]
    pub fn pointed_at(&self, row: u16, column: u16) -> Option<(u16, u16)> {
        let rect = self.surface_rect?;
        let inside = row >= rect.y
            && row < rect.y + rect.height
            && column >= rect.x
            && column < rect.x + rect.width;
        inside.then(|| (row - rect.y, column - rect.x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn span(text: &str) -> Vec<Vec<Span>> {
        vec![vec![Span::new(magi_proto::tooling::Role::Text, text)]]
    }

    #[test]
    fn a_frame_lands_in_the_surface_that_drew_it() {
        let mut app = App::new();
        app.surfaced(
            ToolCallId::new("s0"),
            "dino".to_owned(),
            8,
            "a game".to_owned(),
        );
        app.drew(&ToolCallId::new("s0"), span("running"), None);
        assert_eq!(app.holding().expect("held").drawn, span("running"));
    }

    #[test]
    fn a_frame_for_rows_that_are_gone_is_dropped() {
        let mut app = App::new();
        app.surfaced(
            ToolCallId::new("s1"),
            "dino".to_owned(),
            8,
            "a game".to_owned(),
        );
        app.drew(&ToolCallId::new("s0"), span("stale"), None);
        assert!(app.holding().expect("held").drawn.is_empty());
    }

    #[test]
    fn a_click_arrives_in_the_tenant_own_coordinates() {
        let mut app = App::new();
        app.surface_rect = Some(ratatui::layout::Rect {
            x: 2,
            y: 20,
            width: 40,
            height: 8,
        });
        assert_eq!(app.pointed_at(22, 6), Some((2, 4)));
        // Its own top-left.
        assert_eq!(app.pointed_at(20, 2), Some((0, 0)));
    }

    #[test]
    fn the_pointer_anywhere_else_is_not_the_surface_business() {
        let mut app = App::new();
        app.surface_rect = Some(ratatui::layout::Rect {
            x: 2,
            y: 20,
            width: 40,
            height: 8,
        });
        assert_eq!(app.pointed_at(19, 6), None, "above it");
        assert_eq!(app.pointed_at(28, 6), None, "below it");
        assert_eq!(app.pointed_at(22, 1), None, "left of it");
        assert_eq!(app.pointed_at(22, 42), None, "right of it");
    }

    #[test]
    fn nothing_holding_rows_translates_nothing() {
        // A picker is drawn in the same slot.
        assert_eq!(App::new().pointed_at(22, 6), None);
    }

    #[test]
    fn ending_a_surface_gives_the_rows_back() {
        let mut app = App::new();
        app.surfaced(ToolCallId::new("s0"), "dino".to_owned(), 8, String::new());
        app.unsurfaced(&ToolCallId::new("s0"));
        assert!(app.holding().is_none());
    }

    #[test]
    fn ending_one_that_is_not_on_screen_leaves_the_one_that_is() {
        // Two surfaces in a turn, the first ending after the second opened.
        let mut app = App::new();
        app.surfaced(ToolCallId::new("s1"), "dino".to_owned(), 8, String::new());
        app.unsurfaced(&ToolCallId::new("s0"));
        assert!(app.holding().is_some());
    }
}

/// A question outranking a surface, which is the deadlock this closes.
#[cfg(test)]
mod questioned {
    use crate::app::{App, Picking};
    use magi_proto::ToolCallId;

    /// A tool holding rows, as one drawing a panel does.
    fn holding() -> App {
        let mut app = App::new();
        app.surfaced(
            ToolCallId::new("s0"),
            "casper".to_owned(),
            4,
            "drawing".to_owned(),
        );
        app
    }

    #[test]
    fn a_surface_holds_the_screen_while_nothing_is_waiting() {
        let app = holding();
        assert!(app.holding().is_some());
        assert!(!app.questioned());
    }

    #[test]
    fn a_permission_takes_the_screen_back_from_a_surface() {
        let mut app = holding();
        app.picking = Some(Picking::Permission {
            id: ToolCallId::new("q0"),
            offers: Vec::new(),
        });
        assert!(app.questioned());
        assert!(
            app.holding().is_none(),
            "the surface still owns the menu slot and the keyboard"
        );
    }

    #[test]
    fn a_tools_question_takes_it_too() {
        let mut app = holding();
        app.picking = Some(Picking::Asked {
            id: ToolCallId::new("q1"),
            rows: Vec::new(),
        });
        assert!(app.holding().is_none());
    }

    /// An adoption holds another session's request inside melchior rather than a turn on this
    /// socket — a different caller, stuck the same way.
    #[test]
    fn an_adoption_takes_it_too() {
        let mut app = holding();
        app.picking = Some(Picking::Adoption {
            id: "r1".to_owned(),
        });
        assert!(app.holding().is_none());
    }

    /// A surface drawing under a non-blocking list keeps its rows and its keys.
    #[test]
    fn an_ordinary_list_leaves_a_surface_alone() {
        for picking in [
            Picking::Model,
            Picking::Thinking,
            Picking::Session { rows: Vec::new() },
        ] {
            let mut app = holding();
            app.picking = Some(picking.clone());
            assert!(!app.questioned(), "{picking:?} blocks nobody");
            assert!(app.holding().is_some(), "{picking:?} took the screen");
        }
    }
}
