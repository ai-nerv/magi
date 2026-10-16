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
                self.stack.toggles(*line)
                    || self.stack.line(*line).is_some_and(|drawn| {
                        magi_tui::transcript::hovered(&mut drawn.clone(), *column)
                    })
            });
        was != self.hovering
    }

    /// Fold the block whose handle is at `row`, `column` to its header, or open it again; or show
    /// more or less of the tool block whose row saying so is at `row`.
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
        let Some((nth, _)) = self.stack.at(line) else {
            return false;
        };
        if self.stack.toggles(line) {
            let Some(id) = self.stack.call(nth).cloned() else {
                return false;
            };
            if !self.flipped.remove(&id) {
                self.flipped.insert(id);
            }
            return true;
        }
        if !on_the_handle(self.stack.line(line), column, width) {
            return false;
        }
        if !self.folds.remove(&nth) {
            self.folds.insert(nth);
            self.let_go_of_block(nth);
        }
        true
    }

    /// Give the keyboard back from a running block drawing in block `nth`, now that it is folded.
    fn let_go_of_block(&mut self, nth: usize) {
        let Some(call) = self.stack.call(nth) else {
            return;
        };
        let holds = self.focus.as_ref().is_some_and(|focus| {
            self.running().any(|held| {
                held.id == *focus
                    && !held.pinned
                    && !held.viewing
                    && held.call.as_ref() == Some(call)
            })
        });
        if holds {
            self.let_go();
        }
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
                images: Vec::new(),
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
            for line in 0..self.stack.len() {
                let Some(drawn) = self.stack.line(line) else {
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
        app.lay_out(60);
        app.live_rows = 0..u16::try_from(app.stack.len()).expect("short enough");
        app
    }

    #[test]
    fn a_click_on_a_block_folds_it_and_a_second_click_opens_it() {
        let mut app = laid();
        let (row, column) = app.handle().expect("the block has a handle");
        assert!(app.toggle_at(row, column, 60), "that is the handle");
        assert_eq!(app.folds, [0].into());
        assert!(app.toggle_at(row, column, 60));
        assert!(app.folds.is_empty(), "the same click undoes it");
        assert!(app.flipped.is_empty(), "and neither showed more of it");
    }

    #[test]
    fn folding_a_block_leaves_one_line() {
        let mut app = laid();
        let (row, column) = app.handle().expect("a handle");
        app.toggle_at(row, column, 60);
        app.lay_out(60);
        assert_eq!(app.stack.drawn(), 1, "only the block that was folded");
        assert_eq!(app.scrollback.len(), 1, "its header alone");
    }

    /// Where the row showing more of a block is on screen.
    fn more_row(app: &App) -> u16 {
        (0..app.stack.len())
            .find(|at| app.stack.toggles(*at))
            .and_then(|at| u16::try_from(at).ok())
            .expect("a long result says how much it left")
    }

    #[test]
    fn the_row_saying_how_much_was_left_shows_it_and_then_less() {
        let mut app = laid();
        let short = app.scrollback.len();
        assert!(app.toggle_at(more_row(&app), 0, 60), "that row is a button");
        assert_eq!(app.flipped.len(), 1);
        assert!(app.folds.is_empty(), "showing more is not folding");
        app.lay_out(60);
        assert!(app.scrollback.len() > short, "the whole result is longer");
        app.live_rows = 0..u16::try_from(app.stack.len()).expect("short enough");
        assert!(app.toggle_at(more_row(&app), 0, 60), "and it offers less");
        assert!(app.flipped.is_empty());
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
        assert!(app.folds.is_empty(), "nothing was folded");
        assert!(app.toggle_at(row, column, 60), "and the handle still is");
    }

    #[test]
    fn a_row_of_output_is_not_a_button() {
        // A click on the text of a result is somebody reading it, not somebody folding it.
        let mut app = laid();
        let (row, column) = app.handle().expect("a handle");
        assert!(!app.toggle_at(row + 2, column, 60), "that is output");
        assert!(app.flipped.is_empty() && app.folds.is_empty());
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
        assert!(app.folds.is_empty());
    }

    #[test]
    fn a_message_folds_like_a_call() {
        let mut app = App::new();
        app.entries = vec![Entry::User {
            images: Vec::new(),
            id: MessageId::new("u1".to_owned()),
            text: "a question".to_owned(),
            aside: String::new(),
        }];
        app.lay_out(60);
        app.live_rows = 0..u16::try_from(app.stack.len()).expect("short");
        let (row, column) = app.handle().expect("a message has a handle");
        assert!(
            !app.toggle_at(row + 1, column, 60),
            "its words are not a button"
        );
        assert!(app.toggle_at(row, column, 60));
        assert_eq!(app.folds, [0].into());
    }

    #[test]
    fn a_click_lands_on_the_block_under_it_after_scrolling() {
        // The row is a screen coordinate and the stack's rows are a transcript coordinate; the
        // scroll offset is the whole of the difference between them, and forgetting it makes
        // every click wrong by however far the reader has scrolled.
        let mut app = App::new();
        app.entries = vec![call("t1"), call("t2")];
        app.lay_out(60);
        app.live_rows = 0..10;
        app.scrollback.to_top();

        // One line at a time until a handle is on screen, then click it. Which block it belongs
        // to does not matter; that a screen row still finds the right transcript line does.
        for _ in 0..40 {
            if let Some((row, column)) = app.handle() {
                let hit = app
                    .stack
                    .at(app.scrollback.hidden_above() + usize::from(row))
                    .map(|(nth, _)| nth);
                assert!(app.toggle_at(row, column, 60), "the handle was there");
                assert_eq!(app.folds.len(), 1, "exactly one block was hit");
                assert!(
                    app.folds.contains(&hit.expect("an owner")),
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
        if !on_the_copy(self.stack.line(line), column, width) {
            return None;
        }
        let (nth, _) = self.stack.at(line)?;
        let (_, rows) = self.stack.block(nth)?;
        Some(said_by(rows))
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

/// Every row a block drew, as text, with the chrome taken off.
fn said_by(drawn: &[ratatui::text::Line<'static>]) -> String {
    let mut rows: Vec<String> = Vec::new();
    for drawn in drawn {
        let text: String = drawn
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        // The header and the seams are the block, not what it says.
        let opening = text.trim_start();
        if opening.starts_with(magi_tui::glyph::block_edge())
            || opening.starts_with(magi_tui::glyph::seam().trim())
        {
            continue;
        }
        rows.push(text.trim_end().to_owned());
    }
    // The block's own inset comes off every row at once, so what was indented *within* the block
    // still is. Taking the whitespace off each row separately would flatten a code fence.
    // Counted in characters: an inset of wide or multibyte blanks is not as many bytes as columns.
    let inset = rows
        .iter()
        .filter(|row| !row.trim().is_empty())
        .map(|row| row.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
    let mut out: Vec<String> = rows
        .iter()
        .map(|row| row.chars().skip(inset).collect())
        .collect();
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
        app.lay_out(60);
        app.live_rows = 0..u16::try_from(app.stack.len()).expect("short");
        app
    }

    /// The row and column of the copy chip, found the way a pointer would.
    fn chip(app: &App) -> (u16, u16) {
        for row in 0..app.stack.len() {
            let Some(line) = app.stack.line(row) else {
                continue;
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
    fn a_row_indented_with_multibyte_blanks_is_copied_without_panicking() {
        // An ideographic space is three bytes and one character, a no-break space two: an inset
        // counted in bytes cut the second row in the middle of a character.
        let copied = super::said_by(&[
            ratatui::text::Line::from("  \u{3000}全角の字下げ"),
            ratatui::text::Line::from("  \u{a0}\u{a0}nbsp"),
        ]);
        assert_eq!(copied, "全角の字下げ\n\u{a0}nbsp");
    }

    #[test]
    fn a_seam_inside_the_block_is_not_copied() {
        let mut app = answering("Here is the fix.");
        if let Some(magi_proto::Entry::Assistant { thinking, .. }) = app.entries.first_mut() {
            *thinking = "first, the cause".to_owned();
        }
        app.lay_out(60);
        app.live_rows = 0..u16::try_from(app.stack.len()).expect("short");
        let (row, column) = chip(&app);
        let copied = app.copy_at(row, column, 60).expect("the chip was there");
        assert!(!copied.contains(magi_tui::glyph::seam()), "{copied:?}");
        assert!(copied.contains("Here is the fix."), "{copied:?}");
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
