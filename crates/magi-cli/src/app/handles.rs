//! Folding one tool block at a time.
//!
//! `Ctrl+O` moves the whole transcript between preview and full. This is the other half:
//! the one result you actually want to read is usually not the newest one, and opening
//! everything to reach it buries it again.

use super::App;

impl App {
    /// Fold or unfold the tool block whose handle is at `row`, `column`.
    ///
    /// Returns whether the handle was there. **The handle, not the block.** Every row a block
    /// drew used to answer a click anywhere along it, which made most of the screen a button:
    /// with the mouse captured there was nothing left to aim at, and a click meant to place a
    /// cursor collapsed whatever it landed on. The `»` is the affordance and it is the only
    /// thing that acts.
    pub fn toggle_at(&mut self, row: u16, column: u16, width: u16) -> bool {
        if !self.live_rows.contains(&row) {
            return false;
        }
        let into = usize::from(row - self.live_rows.start);
        let line = self.scrollback.hidden_above() + into;
        let Some(Some(id)) = self.owners.get(line).cloned() else {
            return false;
        };
        if !on_the_handle(self.scrollback.line(line), column, width) {
            return false;
        }
        if !self.flipped.remove(&id) {
            self.flipped.insert(id);
        }
        true
    }
}

/// Whether `column` of this line is the fold handle.
///
/// Asked of the line the renderer produced, not of a rule about where handles go. The handle is
/// right-aligned inside the block's padding today; a renderer that moves it should not have to
/// remember that something over here also knows where it was.
fn on_the_handle(line: Option<&ratatui::text::Line<'static>>, column: u16, width: u16) -> bool {
    let Some(line) = line else {
        return false;
    };
    let mut at = 0_u16;
    for span in &line.spans {
        let wide = u16::try_from(span.content.chars().count()).unwrap_or(0);
        let holds = span.content.contains(magi_tui::glyph::expand())
            || span.content.contains(magi_tui::glyph::collapse());
        if holds && (at..at.saturating_add(wide)).contains(&column) {
            return true;
        }
        at = at.saturating_add(wide);
        if at >= width {
            break;
        }
    }
    false
}
#[cfg(test)]
mod clicking {
    use super::App;
    use magi_proto::{Entry, MessageId, ToolCallId};

    fn call(id: &str) -> Entry {
        Entry::Tool {
            id: ToolCallId::new(id.to_owned()),
            name: "shell".to_owned(),
            args: r#"{"command":"ls"}"#.to_owned(),
            result: Some(magi_proto::ToolResult {
                output: (0..40)
                    .map(|n| format!("line {n}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_error: false,
            }),
            thought_signature: None,
        }
    }

    /// Where the first fold handle is on screen, as `(row, column)`.
    ///
    /// Found by looking, the way the click path finds it, so a test cannot pass by agreeing
    /// with a rule the renderer has stopped following.
    impl App {
        fn handle(&self) -> Option<(u16, u16)> {
            for (line, owner) in self.owners.iter().enumerate() {
                if owner.is_none() {
                    continue;
                }
                let Some(drawn) = self.scrollback.line(line) else {
                    continue;
                };
                let mut at = 0_u16;
                for span in &drawn.spans {
                    if span.content.contains(magi_tui::glyph::expand())
                        || span.content.contains(magi_tui::glyph::collapse())
                    {
                        // Scrolled out of view above: not on screen, so not clickable.
                        let Some(into) = line.checked_sub(self.scrollback.hidden_above()) else {
                            break;
                        };
                        let row = self.live_rows.start + u16::try_from(into).ok()?;
                        if !self.live_rows.contains(&row) {
                            break;
                        }
                        // Where the glyph actually sits inside the chip, rather than a guess at
                        // it: the chip has been ` » ` and is now `[ > ]`, and a helper that knew
                        // the old width aimed at a bracket.
                        let inside = span
                            .content
                            .chars()
                            .position(|c| {
                                magi_tui::glyph::expand().starts_with(c)
                                    || magi_tui::glyph::collapse().starts_with(c)
                            })
                            .and_then(|at| u16::try_from(at).ok())?;
                        return Some((row, at + inside));
                    }
                    at += u16::try_from(span.content.chars().count()).ok()?;
                }
            }
            None
        }
    }

    /// An app with one tool block laid out over rows 0..n.
    fn laid() -> App {
        let mut app = App::new();
        app.entries = vec![call("t1")];
        let laid = magi_tui::transcript::laid_out(
            app.entries(),
            60,
            magi_tui::transcript::Detail::Preview,
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
        let (row, column) = app.handle().expect("the block has a handle");
        assert!(app.toggle_at(row, column, 60), "that is the handle");
        assert_eq!(app.flipped.len(), 1);
        assert!(app.toggle_at(row, column, 60));
        assert!(app.flipped.is_empty(), "the same click undoes it");
    }

    #[test]
    fn unfolding_a_block_makes_it_taller() {
        let mut app = laid();
        let folded = app.scrollback.len();
        let (row, column) = app.handle().expect("a handle");
        app.toggle_at(row, column, 60);
        let opened = magi_tui::transcript::laid_out(
            app.entries(),
            60,
            magi_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        assert!(
            opened.lines.len() > folded,
            "{} is not more than {folded}",
            opened.lines.len()
        );
    }

    #[test]
    fn everything_but_the_handle_is_inert() {
        // The whole point: with the mouse captured, a block that answered a click anywhere
        // along it made most of the screen a button, and a click meant to place a cursor
        // collapsed whatever it landed on.
        let mut app = laid();
        let (row, column) = app.handle().expect("a handle");
        // The chip is the button, brackets included: `[ > ]` is five columns and all of them
        // should act, because aiming at one column is not aiming.
        let chip = column.saturating_sub(2)..=column + 2;
        for at in 0..60_u16 {
            if chip.contains(&at) {
                continue;
            }
            assert!(
                !app.toggle_at(row, at, 60),
                "column {at} of the header should not be a button"
            );
        }
        assert!(app.flipped.is_empty(), "nothing was folded");
        assert!(app.toggle_at(row, column, 60), "and the handle still is");
    }

    #[test]
    fn a_row_of_output_is_not_a_button() {
        // A click on the text of a result is somebody reading it, not somebody folding it.
        let mut app = laid();
        let (row, column) = app.handle().expect("a handle");
        assert!(!app.toggle_at(row + 2, column, 60), "that is output");
        assert!(app.flipped.is_empty());
    }

    #[test]
    fn a_click_outside_the_transcript_does_nothing() {
        let mut app = laid();
        let (_, column) = app.handle().expect("a handle");
        let below = app.live_rows.end + 3;
        assert!(
            !app.toggle_at(below, column, 60),
            "the prompt is not a tool block"
        );
        assert!(app.flipped.is_empty());
    }

    #[test]
    fn a_click_on_something_that_is_not_a_tool_call_does_nothing() {
        let mut app = App::new();
        app.entries = vec![Entry::User {
            id: MessageId::new("u1".to_owned()),
            text: "a question".to_owned(),
            aside: String::new(),
        }];
        let laid = magi_tui::transcript::laid_out(
            app.entries(),
            60,
            magi_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        app.live_rows = 0..u16::try_from(laid.lines.len()).expect("short");
        app.owners = laid.owners;
        assert!(!app.toggle_at(1, 57, 60));
        assert!(app.flipped.is_empty());
    }

    #[test]
    fn a_click_lands_on_the_block_under_it_after_scrolling() {
        // The row is a screen coordinate and the owner list is a transcript coordinate; the
        // scroll offset is the whole of the difference between them, and forgetting it makes
        // every click wrong by however far the reader has scrolled.
        let mut app = App::new();
        app.entries = vec![call("t1"), call("t2")];
        let laid = magi_tui::transcript::laid_out(
            app.entries(),
            60,
            magi_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        app.owners = laid.owners;
        app.scrollback.set_lines(laid.lines);
        app.live_rows = 0..10;
        app.scrollback.to_top();

        // One line at a time until a handle is on screen, then click it. Which block it belongs
        // to does not matter; that a screen row still finds the right transcript line does.
        for _ in 0..40 {
            if let Some((row, column)) = app.handle() {
                let hit = app.owners[app.scrollback.hidden_above() + usize::from(row)].clone();
                assert!(app.toggle_at(row, column, 60), "the handle was there");
                assert_eq!(app.flipped.len(), 1, "exactly one block was hit");
                assert!(
                    app.flipped.contains(&hit.expect("an owner")),
                    "and the right one"
                );
                return;
            }
            app.scrollback.scroll_down(1, 10);
        }
        panic!("no handle came into view");
    }
}
