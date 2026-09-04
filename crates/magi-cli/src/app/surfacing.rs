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
        });
    }

    /// Keep what a surface drew, if it is the one holding the rows.
    ///
    /// A frame for a surface that is not on screen is dropped. It belonged to rows that are gone,
    /// and drawing it into whatever is there now would put one tool's output inside another's.
    pub(super) fn drew(&mut self, id: &ToolCallId, lines: Vec<Vec<Span>>) {
        if let Some(surface) = self.surface.as_mut()
            && surface.id == *id
        {
            surface.drawn = lines;
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
        app.drew(&ToolCallId::new("s0"), span("running"));
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
        app.drew(&ToolCallId::new("s0"), span("stale"));
        assert!(app.holding().expect("held").drawn.is_empty());
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
