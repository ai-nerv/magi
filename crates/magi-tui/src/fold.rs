//! Where a line of prompt text breaks, and where the caret lands once it has.
//!
//! Folding lives apart from drawing because three things have to agree about it: the width the
//! text is wrapped at, the row and column the hardware cursor is placed on, and the number of
//! rows the box asks the layout for. When any one of them measured on its own, a wrapped prompt
//! put the cursor on a row the box was not tall enough to show.

use crate::colour;
use crate::editor::Editor;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

/// Columns a row of text has, once the box and the badge have taken theirs.
///
/// The sides and the padding are three; the badge's strip is the rest. Everything that measures
/// the prompt asks this, so the width the text is folded at and the width it is drawn at cannot
/// drift apart.
#[must_use]
pub fn text_room(width: u16, badge: &str) -> usize {
    let strip = if badge.is_empty() {
        0
    } else {
        badge.chars().count() + 3
    };
    usize::from(width).saturating_sub(3 + strip)
}

/// Every visual row of the editor, and where the cursor sits among them.
pub(crate) fn fold_all(editor: &Editor, room: usize) -> (Vec<String>, usize, usize) {
    let (cursor_row, cursor_col) = editor.cursor();
    let mut visual = Vec::new();
    let (mut caret_row, mut caret_col) = (0, cursor_col);
    for index in 0..editor.lines().len() {
        let text = crate::prompt::resolving(editor, index);
        if index == cursor_row {
            let (row, col) = folded_cursor(&text, room, cursor_col);
            caret_row = visual.len() + row;
            caret_col = col;
        }
        visual.extend(folded(&text, room));
    }
    (visual, caret_row, caret_col)
}

/// Where the caret is, in folded rows and columns.
///
/// For the terminal's own cursor, which has to land on the cell the inverted block is drawn on.
#[must_use]
pub fn caret(editor: &Editor, width: u16, badge: &str) -> (usize, usize) {
    let (_, row, col) = fold_all(editor, text_room(width, badge));
    (row, col)
}

/// Break one logical line into the visual rows it occupies, at `width`.
///
/// The prompt used to draw a logical line as one row however long it was, which ran the text
/// straight through the right-hand border and off the screen. It has to wrap now that a badge
/// sits in the box: text going under the badge would be worse than text running off the edge,
/// because the badge is the thing that says which session you are typing into.
///
/// On a word boundary where there is one, and mid-word only for a word longer than the whole
/// width -- a path or a URL, which is exactly the case where breaking anywhere is right.
#[must_use]
pub(crate) fn folded(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_owned()];
    }
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in text.split_inclusive(' ') {
        if word.trim_end().chars().count() > width {
            for ch in word.chars() {
                if row.chars().count() == width {
                    rows.push(std::mem::take(&mut row));
                }
                row.push(ch);
            }
            continue;
        }
        if row.chars().count() + word.trim_end().chars().count() > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        row.push_str(word);
    }
    rows.push(row);
    rows
}

/// Where the cursor lands once `text` is folded at `width`.
///
/// Returned as `(row, column)` among the folded rows, because a caret counted along a logical
/// line means nothing once that line is three rows on the screen.
#[must_use]
pub(crate) fn folded_cursor(text: &str, width: usize, col: usize) -> (usize, usize) {
    let rows = folded(text, width);
    let mut left = col;
    for (at, row) in rows.iter().enumerate() {
        let held = row.chars().count();
        if left <= held || at + 1 == rows.len() {
            return (at, left.min(held));
        }
        left -= held;
    }
    (0, col)
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    /// `(char, reversed)` for each cell of the placeholder row.
    fn cells(hint: &str, caret: Option<usize>) -> Vec<(String, bool)> {
        crate::prompt::placeholder_spans(
            60,
            &crate::tease::Saying {
                text: hint,
                caret,
                block: true,
                ..Default::default()
            },
        )
        .into_iter()
        .map(|s| {
            (
                s.content.into_owned(),
                s.style.add_modifier.contains(Modifier::REVERSED),
            )
        })
        .collect()
    }

    #[test]
    fn the_real_cursor_sits_on_the_first_letter() {
        // Not in front of it. It marks where typing would land, and typing lands on column
        // zero whatever the box happens to be saying.
        let row = cells("build", None);
        assert_eq!(row[0], ("b".to_owned(), true), "{row:?}");
        assert_eq!(row[1], ("u".to_owned(), false), "{row:?}");
    }

    #[test]
    fn the_real_cursor_stays_put_wherever_the_other_one_is() {
        // The white block is yours. A cursor that wandered off while the box amused itself
        // would be telling you your text was going somewhere else.
        for caret in [None, Some(1), Some(3), Some(5)] {
            let row = cells("build", caret);
            assert!(row[0].1, "the first cell lost its cursor at {caret:?}");
        }
    }

    #[test]
    fn the_writing_cursor_is_where_the_editing_is() {
        let row = cells("build", Some(3));
        assert!(row[3].1, "nothing marked at three: {row:?}");
        assert!(!row[2].1, "and only there: {row:?}");
        assert!(!row[4].1, "{row:?}");
    }

    #[test]
    fn it_can_sit_past_the_last_letter() {
        // Which is where it is while text is being added to the end -- most of the time.
        let row = cells("build", Some(5));
        assert_eq!(row.len(), 6, "a cell was not added for it: {row:?}");
        assert_eq!(row[5], (" ".to_owned(), true));
    }

    #[test]
    fn resting_shows_only_your_own() {
        let row = cells("build", None);
        assert_eq!(row.iter().filter(|(_, on)| *on).count(), 1);
    }

    #[test]
    fn an_empty_line_still_has_a_cursor() {
        // A box with nothing in it and no cursor reads as a screen that has hung.
        let row = cells("", None);
        assert_eq!(row, vec![(" ".to_owned(), true)]);
    }
}

/// The strip down the right of the box, and what sits in it on this row.
///
/// **Reserved on every row, not just the one it is on.** A badge that only shortened its own row
/// would let the text above it run the full width, and the block would reflow every time the
/// prompt grew past a line — the right-hand edge of what you are typing would move while you
/// typed. A constant margin costs a few columns and never moves.
///
/// The badge sits on the middle row, rounding down for an even count, so a one-line prompt has it
/// beside the text and a tall one has it level with the middle rather than stuck to a corner.
/// `rows` is the *text* rows: a menu opened under the divider is not part of the box you type in.
pub(crate) fn strip(badge: &str, rows: usize, row: usize) -> Vec<Span<'static>> {
    if badge.is_empty() {
        return Vec::new();
    }
    // A padded space each side of the name, inverted along with it so it reads as one block, and
    // a plain one after so the block does not sit against the border.
    let worn = badge.chars().count() + 3;
    if row != rows / 2 {
        return vec![Span::raw(" ".repeat(worn))];
    }
    vec![
        Span::styled(
            format!(" {badge} "),
            Style::default()
                .fg(colour::hint())
                .add_modifier(Modifier::REVERSED),
        ),
        Span::raw(" "),
    ]
}

/// The box says which session you are typing into, and the text never runs under it.
#[cfg(test)]
mod badge_tests {
    use super::*;

    const NAME: &str = "axum/main/alpha";

    /// The prompt drawn at `width` with `text` in it, as plain rows.
    fn rows_with(text: &str, width: u16) -> Vec<String> {
        let mut editor = Editor::new();
        editor.insert_str(text);
        crate::prompt::render(
            &editor,
            width,
            24,
            0,
            crate::border::Scan::Off,
            &[],
            crate::tease::Saying {
                badge: NAME,
                ..Default::default()
            },
        )
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
    }

    #[test]
    fn long_text_wraps_rather_than_running_under_the_badge() {
        // The whole point. It used to draw a logical line as one row however long it was, which
        // ran the text through the right-hand border and off the screen.
        let rows = rows_with(
            "this is a long prompt that certainly runs past the width of the box",
            50,
        );
        for row in &rows {
            assert_eq!(row.chars().count(), 50, "{row:?} is not the box width");
        }
        assert!(rows.len() > 3, "it did not wrap: {rows:#?}");
    }

    #[test]
    fn the_badge_is_on_the_middle_row_rounding_down() {
        // One row has it beside the text; a tall one has it level with the middle rather than
        // stuck to a corner.
        for (text, want) in [
            ("short", 0usize),
            ("a line long enough to take exactly two rows here", 1),
        ] {
            let rows = rows_with(text, 50);
            // Row 0 is the top edge, so the text rows start at 1.
            let text_rows = rows.len() - 2;
            let at = rows
                .iter()
                .position(|r| r.contains(NAME))
                .expect("the badge is drawn");
            assert_eq!(
                at - 1,
                (text_rows / 2).min(want.max(text_rows / 2)),
                "{rows:#?}"
            );
        }
    }

    #[test]
    fn every_row_reserves_the_strip_even_where_the_badge_is_not() {
        // A margin only on the badge's own row would let the text above it run the full width,
        // and the block would reflow every time the prompt grew past a line.
        let rows = rows_with("one\ntwo\nthree", 50);
        let wide = NAME.chars().count() + 3;
        let worn = format!(" {NAME}  ");
        for row in &rows[1..rows.len() - 1] {
            // The strip is the last `wide` columns before the right border.
            let cells: Vec<char> = row.chars().collect();
            let strip: String = cells[cells.len() - 1 - wide..cells.len() - 1]
                .iter()
                .collect();
            assert!(
                strip == worn || strip.chars().all(|c| c == ' '),
                "{strip:?} is neither the badge nor empty"
            );
        }
    }

    #[test]
    fn a_word_longer_than_the_row_is_broken_rather_than_lost() {
        // A path or a URL, which is exactly the case where breaking anywhere is right.
        // `z`, because the badge has letters in it and this counts occurrences.
        let long = "z".repeat(90);
        let rows = rows_with(&long, 50);
        let joined: String = rows[1..rows.len() - 1].concat();
        assert_eq!(joined.matches('z').count(), 90, "characters went missing");
    }

    #[test]
    fn no_badge_gives_the_text_the_whole_width() {
        // Nothing reserves a strip that nothing is going to sit in.
        let with = text_room(50, NAME);
        let without = text_room(50, "");
        assert!(with < without);
        assert_eq!(without, 47, "the sides and the padding, and nothing else");
    }

    #[test]
    fn the_caret_follows_the_text_around_a_fold() {
        // The terminal's own cursor has to land on the cell the inverted block is drawn on, and
        // a caret counted along a logical line is on the wrong row once that line wraps.
        let mut editor = Editor::new();
        editor.insert_str(&"b".repeat(60));
        let (row, col) = caret(&editor, 50, NAME);
        assert!(row > 0, "the caret stayed on the first row");
        assert!(col <= text_room(50, NAME), "it is off the right edge");
    }
}
