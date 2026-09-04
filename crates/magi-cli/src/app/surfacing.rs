//! Rows a tool is holding, and what it last drew in them.
//!
//! The UI's whole share of a surface. It reserves the space, keeps the most recent frame, and
//! sends keys to whoever holds it — and it never reads what is in there. A permission prompt, a
//! file picker and a game are the same three fields.

use magi_proto::ToolCallId;
use magi_proto::tooling::Span;

/// A surface currently on the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Surfacing {
    /// Which surface this is, so keys reach it and its frames land in it.
    pub id: ToolCallId,
    /// The tool holding it, for a footer that says who has the screen.
    pub tool: String,
    /// How many rows were reserved.
    ///
    /// Kept even though the frames say how many they drew: the reservation is what the layout is
    /// built from, and a surface whose height changed with every frame would make the transcript
    /// above it jump each time its tenant drew a shorter one.
    pub rows: u16,
    /// What it is for, shown until it draws its first frame.
    pub about: String,
    /// The last frame it drew.
    pub drawn: Vec<Vec<Span>>,
    /// Where it asked for the terminal's own cursor, in its own coordinates.
    ///
    /// `None` for almost every surface: a game paints its own picture and wants nothing blinking
    /// in it. A tenant that draws a field somebody types into asks for it, and then the caret an
    /// IME and a screen reader follow is in the field rather than back in the prompt.
    pub cursor: Option<magi_proto::tooling::At>,
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

    /// Keep what a surface drew, if it is the one holding the rows.
    ///
    /// A frame for a surface that is not on screen is dropped. It belonged to rows that are gone,
    /// and drawing it into whatever is there now would put one tool's output inside another's.
    pub(super) fn drew(
        &mut self,
        id: &ToolCallId,
        lines: Vec<Vec<Span>>,
        cursor: Option<magi_proto::tooling::At>,
    ) {
        if let Some(surface) = self.surface.as_mut()
            && surface.id == *id
        {
            surface.drawn = lines;
            // Per frame, like the rows: a caret that stayed where the last frame put it would be
            // one a tenant could never take back.
            surface.cursor = cursor;
        }
    }

    /// Take the rows back.
    pub(super) fn unsurfaced(&mut self, id: &ToolCallId) {
        if self.surface.as_ref().is_some_and(|held| held.id == *id) {
            self.surface = None;
        }
    }

    /// The surface holding the rows, when one is.
    #[must_use]
    pub fn holding(&self) -> Option<&Surfacing> {
        self.surface.as_ref()
    }

    /// Turn a screen cell into one of the tenant's own, when it landed on its rows.
    ///
    /// `None` for anywhere else, and that is the whole of the filtering: a surface hears about the
    /// pointer over its rows and about nothing else on the screen. Clicking the transcript above
    /// it is the transcript's business, and forwarding it would let a tenant watch the pointer
    /// wander around a window it was given eight rows of.
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
        // It belonged to a surface that has ended. Drawing it into whatever holds the rows now
        // would put one tool's output inside another's.
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
        // The whole of magi's share of the pointer: it knows where it drew the rows, and turns a
        // screen cell into one of the tenant's. The tenant is never told the other half.
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
        // A surface hears about its own rows and about nothing else on the screen. Forwarding the
        // rest would let a tenant granted eight rows watch the pointer cross the whole window.
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
        // A picker is drawn in the same slot. A click on one must not arrive as coordinates for
        // a surface that closed.
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
        // Two surfaces in a turn, the first ending after the second opened. Taking the rows on
        // any `Unsurfaced` would blank a surface that is still being played.
        let mut app = App::new();
        app.surfaced(ToolCallId::new("s1"), "dino".to_owned(), 8, String::new());
        app.unsurfaced(&ToolCallId::new("s0"));
        assert!(app.holding().is_some());
    }
}
