//! Selecting text with the mouse.
//!
//! **Why magi does this itself.** Mouse reporting is one terminal-wide switch. An application
//! that turns it on to receive a click stops the terminal running its own drag-selection, and no
//! choice of tracking mode changes that: the terminal is not deciding per-region, it is deciding
//! whether the application gets the mouse at all. So a program that wants both a clickable
//! element and selectable text has exactly one option, which is to select the text itself. That
//! is what neovim does, and tmux, and every full-screen program in the same position.
//!
//! What that buys beyond parity: the selection is over the transcript magi rendered, so it knows
//! where a line ends and does not take the padding with it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

/// A drag in progress, or a finished one, in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Where the press landed.
    anchor: (u16, u16),
    /// Where the pointer is now.
    head: (u16, u16),
    /// Whether the button is still down.
    dragging: bool,
}

impl Selection {
    /// Begin a drag at `row`, `column`.
    #[must_use]
    pub fn begin(row: u16, column: u16) -> Self {
        Self {
            anchor: (row, column),
            head: (row, column),
            dragging: true,
        }
    }

    /// Move the loose end.
    pub fn drag_to(&mut self, row: u16, column: u16) {
        self.head = (row, column);
    }

    /// The button came up.
    pub fn finish(&mut self) {
        self.dragging = false;
    }

    /// Whether anything is actually covered.
    ///
    /// A press with no drag is a click, not a selection, and highlighting the single cell under
    /// a click makes every click leave a mark behind.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The two ends in reading order, whichever way the drag went.
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether this cell is inside the selection.
    ///
    /// Linewise between the ends rather than a rectangle: dragging down three lines of a
    /// paragraph selects the paragraph, which is what a selection means everywhere else.
    #[must_use]
    pub fn covers(&self, row: u16, column: u16) -> bool {
        if self.is_empty() {
            return false;
        }
        let ((top, from), (bottom, to)) = self.ordered();
        if row < top || row > bottom {
            return false;
        }
        let after_start = row > top || column >= from;
        let before_end = row < bottom || column < to;
        after_start && before_end
    }
}

/// Paint the selection over the finished frame.
///
/// Reversed rather than recoloured: the transcript already spends its palette on what a line
/// means, and a selection that repainted the foreground would erase the difference between a
/// diff's additions and its removals exactly where somebody is looking hardest.
pub fn over(buffer: &mut Buffer, selection: Selection) {
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if selection.covers(y, x) {
                let cell = &mut buffer[(x, y)];
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

/// The text the selection covers, read back out of the frame.
///
/// Read from the buffer rather than from the transcript it was rendered from, because what a
/// person dragged over is what they saw: wrapped where it wrapped, and without the parts of a
/// line that scrolled off the side.
///
/// Trailing blanks go. A block's background runs to the edge of the screen, so every line of one
/// ends in padding nobody meant to copy.
#[must_use]
pub fn text(buffer: &Buffer, selection: Selection, area: Rect) -> String {
    let ((top, _), (bottom, _)) = selection.ordered();
    let mut out = String::new();
    for y in top.max(area.top())..=bottom.min(area.bottom().saturating_sub(1)) {
        let mut line = String::new();
        for x in area.left()..area.right() {
            if selection.covers(y, x) {
                line.push_str(buffer[(x, y)].symbol());
            }
        }
        out.push_str(line.trim_end());
        if y < bottom {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which buffer a finished frame's text has to be read out of.
    ///
    /// The copy was silently empty for as long as it existed, and nothing here could see it:
    /// every other test in this file builds a `Buffer` by hand, so the text extraction was right
    /// and the *source* was wrong. This is the missing half.
    mod out_of_the_frame_that_was_drawn {
        use super::*;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::text::Line;
        use ratatui::widgets::Paragraph;

        /// Draw one line of text and hand back the terminal and what `draw` reported.
        fn drawn() -> (Terminal<TestBackend>, String, Selection) {
            let mut terminal = Terminal::new(TestBackend::new(20, 3)).expect("a terminal");
            let completed = terminal
                .draw(|frame| {
                    frame.render_widget(Paragraph::new(Line::from("hello there")), frame.area());
                })
                .expect("a frame");
            let mut sel = Selection::begin(0, 0);
            sel.drag_to(0, 11);
            sel.finish();
            let from_frame = text(completed.buffer, sel, completed.area);
            (terminal, from_frame, sel)
        }

        #[test]
        fn the_frame_that_was_drawn_still_holds_its_text() {
            let (_terminal, from_frame, _) = drawn();
            assert_eq!(from_frame, "hello there");
        }

        #[test]
        fn the_current_buffer_after_a_draw_is_blank() {
            // The trap, written down. `draw` ends with `swap_buffers`, which *resets* the buffer
            // it is about to make current -- so asking the terminal for its "current" buffer
            // after drawing hands back an empty one. Reading the selection from there produced
            // the empty string every time, and an empty string was taken for "nothing was
            // selected" rather than for "we looked in the wrong place".
            let (mut terminal, _, sel) = drawn();
            let area = terminal.get_frame().area();
            let from_current = text(terminal.current_buffer_mut(), sel, area);
            assert!(
                from_current.trim().is_empty(),
                "if this ever holds the text, the reason for reading the frame is gone: \
                 {from_current:?}"
            );
        }
    }

    #[test]
    fn a_press_with_no_drag_covers_nothing() {
        // Otherwise every click leaves a mark on the cell under it.
        let one = Selection::begin(3, 10);
        assert!(one.is_empty());
        assert!(!one.covers(3, 10));
    }

    #[test]
    fn a_drag_along_one_line_covers_what_it_crossed() {
        let mut sel = Selection::begin(3, 4);
        sel.drag_to(3, 9);
        assert!(!sel.covers(3, 3));
        assert!(sel.covers(3, 4));
        assert!(sel.covers(3, 8));
        assert!(
            !sel.covers(3, 9),
            "the end is where the pointer is, exclusive"
        );
        assert!(!sel.covers(4, 5), "and it is one line");
    }

    #[test]
    fn a_drag_across_lines_is_linewise_not_rectangular() {
        // Dragging down three lines of a paragraph selects the paragraph. A rectangle would
        // take a column out of the middle of it, which is not what a selection means anywhere.
        let mut sel = Selection::begin(2, 40);
        sel.drag_to(4, 2);
        assert!(sel.covers(2, 40), "from the anchor to the end of its line");
        assert!(sel.covers(2, 70));
        assert!(sel.covers(3, 0), "the whole of the line between");
        assert!(sel.covers(3, 70));
        assert!(sel.covers(4, 1), "and up to the head on the last");
        assert!(!sel.covers(4, 2));
        assert!(!sel.covers(1, 40), "nothing above");
        assert!(!sel.covers(5, 0), "nothing below");
    }

    #[test]
    fn dragging_backwards_selects_the_same_thing() {
        let mut down = Selection::begin(2, 5);
        down.drag_to(4, 8);
        let mut up = Selection::begin(4, 8);
        up.drag_to(2, 5);
        for row in 1..6_u16 {
            for column in 0..12_u16 {
                assert_eq!(
                    down.covers(row, column),
                    up.covers(row, column),
                    "{row},{column}"
                );
            }
        }
    }

    /// A buffer with `rows` written into it, one a line.
    fn drawn(rows: &[&str]) -> Buffer {
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut buffer = Buffer::empty(Rect::new(
            0,
            0,
            u16::try_from(width).expect("narrow"),
            u16::try_from(rows.len()).expect("short"),
        ));
        for (y, row) in rows.iter().enumerate() {
            buffer.set_string(
                0,
                u16::try_from(y).expect("short"),
                row,
                ratatui::style::Style::default(),
            );
        }
        buffer
    }

    #[test]
    fn the_text_is_what_was_covered() {
        let buffer = drawn(&["hello world", "second line"]);
        let mut sel = Selection::begin(0, 6);
        sel.drag_to(0, 11);
        assert_eq!(text(&buffer, sel, buffer.area), "world");
    }

    #[test]
    fn several_lines_come_back_with_the_newlines_between_them() {
        let buffer = drawn(&["alpha", "beta", "gamma"]);
        let mut sel = Selection::begin(0, 0);
        sel.drag_to(2, 5);
        assert_eq!(text(&buffer, sel, buffer.area), "alpha\nbeta\ngamma");
    }

    #[test]
    fn the_padding_a_block_draws_is_not_copied() {
        // A tool block's background runs to the edge of the screen, so every line of one ends
        // in spaces nobody dragged over on purpose.
        let buffer = drawn(&["text      ", "more      "]);
        let mut sel = Selection::begin(0, 0);
        sel.drag_to(1, 10);
        assert_eq!(text(&buffer, sel, buffer.area), "text\nmore");
    }

    #[test]
    fn the_selection_is_reversed_rather_than_recoloured() {
        // The transcript spends its palette on what a line means -- a diff's additions against
        // its removals -- and repainting the foreground would erase that where it is read most.
        let mut buffer = drawn(&["hello"]);
        let mut sel = Selection::begin(0, 0);
        sel.drag_to(0, 3);
        over(&mut buffer, sel);
        assert!(
            buffer[(0, 0)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !buffer[(4, 0)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }
}
