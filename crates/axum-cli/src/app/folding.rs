//! Folding one tool block at a time.
//!
//! `Ctrl+O` moves the whole transcript between preview and full. This is the other half:
//! the one result you actually want to read is usually not the newest one, and opening
//! everything to reach it buries it again.

use super::App;

impl App {
    /// Fold or unfold the tool block drawn on screen row `row`.
    ///
    /// Returns whether anything was on that row. A click on prose, on a blank line between
    /// blocks, or below the transcript does nothing — the alternative is a click anywhere near a
    /// block collapsing it, which makes selecting text feel like the UI is fighting you.
    pub fn toggle_at(&mut self, row: u16) -> bool {
        if !self.live_rows.contains(&row) {
            return false;
        }
        let into = usize::from(row - self.live_rows.start);
        let line = self.scrollback.hidden_above() + into;
        let Some(Some(id)) = self.owners.get(line).cloned() else {
            return false;
        };
        if !self.flipped.remove(&id) {
            self.flipped.insert(id);
        }
        true
    }
}

#[cfg(test)]
mod clicking {
    use super::App;
    use axum_proto::{Entry, MessageId, ToolCallId};

    fn call(id: &str) -> Entry {
        Entry::Tool {
            id: ToolCallId::new(id.to_owned()),
            name: "shell".to_owned(),
            args: r#"{"command":"ls"}"#.to_owned(),
            result: Some(axum_proto::ToolResult {
                output: (0..40)
                    .map(|n| format!("line {n}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_error: false,
            }),
            thought_signature: None,
        }
    }

    /// An app with one tool block laid out over rows 0..n.
    fn laid() -> App {
        let mut app = App::new();
        app.entries = vec![call("t1")];
        let laid = axum_tui::transcript::laid_out(
            app.entries(),
            60,
            axum_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        app.live_rows = 0..u16::try_from(laid.lines.len()).expect("short enough");
        app.owners = laid.owners;
        app.scrollback.set_lines(laid.lines);
        app
    }

    #[test]
    fn a_click_on_a_block_flips_it_and_a_second_click_puts_it_back() {
        let mut app = laid();
        assert!(app.toggle_at(1), "row 1 is inside the block");
        assert_eq!(app.flipped.len(), 1);
        assert!(app.toggle_at(1));
        assert!(app.flipped.is_empty(), "the same click undoes it");
    }

    #[test]
    fn unfolding_a_block_makes_it_taller() {
        let mut app = laid();
        let folded = app.scrollback.len();
        app.toggle_at(1);
        let opened = axum_tui::transcript::laid_out(
            app.entries(),
            60,
            axum_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        assert!(
            opened.lines.len() > folded,
            "{} is not more than {folded}",
            opened.lines.len()
        );
    }

    #[test]
    fn a_click_outside_the_transcript_does_nothing() {
        let mut app = laid();
        let below = app.live_rows.end + 3;
        assert!(!app.toggle_at(below), "the prompt is not a tool block");
        assert!(app.flipped.is_empty());
    }

    #[test]
    fn a_click_on_something_that_is_not_a_tool_call_does_nothing() {
        let mut app = App::new();
        app.entries = vec![Entry::User {
            id: MessageId::new("u1".to_owned()),
            text: "a question".to_owned(),
        }];
        let laid = axum_tui::transcript::laid_out(
            app.entries(),
            60,
            axum_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        app.live_rows = 0..u16::try_from(laid.lines.len()).expect("short");
        app.owners = laid.owners;
        assert!(!app.toggle_at(1));
        assert!(app.flipped.is_empty());
    }

    #[test]
    fn a_click_lands_on_the_block_under_it_after_scrolling() {
        // The row is a screen coordinate and the owner list is a transcript coordinate; the
        // scroll offset is the whole of the difference between them, and forgetting it makes
        // every click wrong by however far the reader has scrolled.
        let mut app = App::new();
        app.entries = vec![call("t1"), call("t2")];
        let laid = axum_tui::transcript::laid_out(
            app.entries(),
            60,
            axum_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        let first = laid.lines.len();
        app.owners = laid.owners;
        app.scrollback.set_lines(laid.lines);
        app.live_rows = 0..10;
        app.scrollback.scroll_up(0);
        // Scrolled so the second block's first line sits on the top row.
        app.scrollback.to_top();
        for _ in 0..first {
            app.scrollback.scroll_down(1, 10);
        }
        app.toggle_at(0);
        assert_eq!(app.flipped.len(), 1, "exactly one block was hit");
    }
}
