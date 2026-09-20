//! Folding one tool block at a time.
//!
//! `Ctrl+O` moves the whole transcript between preview and full. This is the other half:
//! the one result you actually want to read is usually not the newest one, and opening
//! everything to reach it buries it again.

use super::App;

impl App {
    /// Note where the pointer is, and say whether the screen has to be redrawn.
    ///
    /// **Only when the answer changes.** Any-event tracking reports a message per cell the
    /// pointer crosses, and redrawing for each would repaint the screen continuously while
    /// somebody moves the mouse across it. Almost every one of those messages lands on the same
    /// answer as the last — usually "over no handle at all" — and this is what makes them free.
    pub fn hover_at(&mut self, row: u16, column: u16) -> bool {
        let was = self.hovering;
        self.hovering = self
            .live_rows
            .contains(&row)
            .then(|| {
                let into = usize::from(row - self.live_rows.start);
                (self.scrollback.hidden_above() + into, column)
            })
            // A pointer that has left the transcript lights nothing, and the handle it was over
            // has to go dark: a highlight left behind points at something nobody is aiming at.
            .filter(|(line, column)| {
                self.scrollback
                    .line(*line)
                    .is_some_and(|drawn| magi_tui::transcript::hovered(&mut drawn.clone(), *column))
            });
        was != self.hovering
    }

    /// Fold or unfold the tool block whose handle is at `row`, `column`.
    ///
    /// Returns whether the handle was there. **The handle, not the block.** Every row a block
    /// drew used to answer a click anywhere along it, which made most of the screen a button:
    /// with the mouse captured there was nothing left to aim at, and a click meant to place a
    /// cursor collapsed whatever it landed on. The chip is the affordance and it is the only
    /// thing that acts — which is also what lights up when the pointer is on it.
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
                shown: None,
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

/// Putting what a block says on the clipboard.
impl App {
    /// The text of the block whose copy chip is at `row`, `column`.
    ///
    /// `None` when the pointer is not on a chip. **As drawn, minus the chrome**: the frame, the
    /// padding rows and the block's own left inset come off, and what is left is what a person
    /// sees — wrapped where the screen wrapped it, because that is what they are pointing at.
    pub fn copy_at(&self, row: u16, column: u16, width: u16) -> Option<String> {
        if !self.live_rows.contains(&row) {
            return None;
        }
        let into = usize::from(row - self.live_rows.start);
        let line = self.scrollback.hidden_above() + into;
        if !on_the_copy(self.scrollback.line(line), column, width) {
            return None;
        }
        let block = (*self.blocks.get(line)?)?;
        Some(said_by(&self.scrollback, &self.blocks, block))
    }
}

/// Whether `column` of this line is the copy chip.
///
/// Asked of the line the renderer produced, like the fold handle next door: where a chip sits is
/// the renderer's business, and a second place that knew would be a second place to fix.
fn on_the_copy(line: Option<&ratatui::text::Line<'static>>, column: u16, width: u16) -> bool {
    let Some(line) = line else {
        return false;
    };
    let mut at = 0_u16;
    for span in &line.spans {
        let wide = u16::try_from(span.content.chars().count()).unwrap_or(0);
        if span.content.contains(magi_tui::glyph::copy())
            && (at..at.saturating_add(wide)).contains(&column)
        {
            return true;
        }
        at = at.saturating_add(wide);
        if at >= width {
            break;
        }
    }
    false
}

/// Every row `block` drew, as text, with the chrome taken off.
fn said_by(
    scrollback: &magi_tui::scrollback::Scrollback,
    blocks: &[Option<usize>],
    block: usize,
) -> String {
    let mut rows: Vec<String> = Vec::new();
    for (line, owner) in blocks.iter().enumerate() {
        if *owner != Some(block) {
            continue;
        }
        let Some(drawn) = scrollback.line(line) else {
            continue;
        };
        let text: String = drawn
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        // The edges are the block, not what it says.
        if text.trim_start().starts_with('┌') || text.trim_start().starts_with('└') {
            continue;
        }
        rows.push(text.trim_end().to_owned());
    }
    // The block's own inset comes off every row at once, so what was indented *within* the block
    // still is. Taking the whitespace off each row separately would flatten a code fence.
    let inset = rows
        .iter()
        .filter(|row| !row.trim().is_empty())
        .map(|row| row.len() - row.trim_start().len())
        .min()
        .unwrap_or(0);
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
    let mut out: Vec<&str> = Vec::new();
    for row in &rows {
        out.push(if row.len() >= inset {
            &row[inset..]
        } else {
            ""
        });
    }
    while out.first().is_some_and(|row| row.trim().is_empty()) {
        out.remove(0);
    }
    out.join("\n")
}

/// What a copy chip puts on the clipboard.
#[cfg(test)]
mod copying {
    use super::App;
    use magi_proto::{Entry, MessageId};

    /// An app showing one assistant answer, laid out and ready to be clicked.
    fn answering(text: &str) -> App {
        let mut app = App::new();
        app.entries = vec![Entry::Assistant {
            id: MessageId::new("m1"),
            text: text.to_owned(),
            thinking: String::new(),
            stop_reason: None,
            error: None,
            usage: magi_proto::Usage::default(),
            signatures: magi_proto::Signatures::default(),
        }];
        let laid = magi_tui::transcript::laid_out(
            app.entries(),
            60,
            magi_tui::transcript::Detail::Preview,
            &app.flipped,
        );
        app.owners = laid.owners;
        app.blocks = laid.blocks;
        app.live_rows = 0..u16::try_from(laid.lines.len()).expect("short");
        app.scrollback.set_lines(laid.lines);
        app
    }

    /// The row and column of the copy chip, found the way a pointer would.
    fn chip(app: &App) -> (u16, u16) {
        for row in 0..1000 {
            let Some(line) = app.scrollback.line(row) else {
                break;
            };
            let mut at = 0_u16;
            for span in &line.spans {
                if span.content.contains(magi_tui::glyph::copy()) {
                    return (u16::try_from(row).expect("short"), at);
                }
                at += u16::try_from(span.content.chars().count()).expect("short");
            }
        }
        panic!("no copy chip was drawn");
    }

    #[test]
    fn the_chip_copies_what_the_block_says_and_not_its_frame() {
        let app = answering("Here is the fix.");
        let (row, column) = chip(&app);
        let copied = app.copy_at(row, column, 60).expect("the chip was there");
        assert_eq!(copied, "Here is the fix.");
    }

    #[test]
    fn indentation_within_the_block_survives_the_inset_coming_off() {
        // The block's own left margin comes off every row at once. Taken off each row separately
        // it would flatten a code fence into prose, which is the one thing a copy is for.
        let app = answering("Try this:\n\n    let x = 1;");
        let (row, column) = chip(&app);
        let copied = app.copy_at(row, column, 60).expect("the chip was there");
        assert!(copied.contains("    let x = 1;"), "{copied:?}");
        assert!(copied.starts_with("Try this:"), "{copied:?}");
    }

    #[test]
    fn a_press_that_is_not_on_the_chip_copies_nothing() {
        // Otherwise the whole edge is a button, and a click meant to place a cursor quietly
        // replaces whatever was on the clipboard.
        let app = answering("Here is the fix.");
        let (row, column) = chip(&app);
        assert!(app.copy_at(row, column.saturating_sub(4), 60).is_none());
    }
}
