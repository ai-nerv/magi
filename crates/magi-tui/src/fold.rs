//! Where a line of prompt text breaks, and where the caret lands once it has. Apart from drawing
//! because the fold width, the hardware cursor's row and column, and the rows the box asks the
//! layout for all have to agree, or the cursor lands on a row the box is not tall enough to show.

use crate::colour;
use crate::editor::Editor;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

/// Columns a row of text has once the box and the badge have taken theirs: three for the sides and
/// padding, the rest for the badge's strip. Everything that measures the prompt asks this.
#[must_use]
pub fn text_room(width: u16, badge: &str) -> usize {
    let strip = if badge.is_empty() {
        0
    } else {
        badge.chars().count() + 3
    };
    usize::from(width).saturating_sub(3 + strip)
}

/// Line `row` with anything typed a moment ago still on its way to being itself. A character arrives
/// as the first of [`crate::glyph::type_stages`], passes through the rest and lands as what was
/// typed. Off unless `magi.ui.type_reveal_ms` says otherwise, and the same width throughout.
pub(crate) fn resolving(editor: &Editor, row: usize) -> String {
    let text = &editor.lines()[row];
    let over = crate::metric::type_reveal_ms();
    let stages: Vec<char> = crate::glyph::type_stages().chars().collect();
    if over == 0 || stages.is_empty() {
        return text.clone();
    }
    let each = (over / stages.len() as u64).max(1);
    text.char_indices()
        .enumerate()
        .map(|(col, (_, ch))| {
            let Some(age) = editor.typed_age(row, col, ch) else {
                return ch;
            };
            let stage = usize::try_from(age.as_millis() / u128::from(each)).unwrap_or(usize::MAX);
            stages.get(stage).copied().unwrap_or(ch)
        })
        .collect()
}

/// Every visual row of the editor, and where the cursor sits among them.
pub(crate) fn fold_all(editor: &Editor, room: usize) -> (Vec<String>, usize, usize) {
    let (cursor_row, cursor_col) = editor.cursor();
    let mut visual = Vec::new();
    let (mut caret_row, mut caret_col) = (0, cursor_col);
    for index in 0..editor.lines().len() {
        let text = resolving(editor, index);
        if index == cursor_row {
            let (row, col) = folded_cursor(&text, room, cursor_col);
            caret_row = visual.len() + row;
            caret_col = col;
        }
        visual.extend(folded(&text, room));
    }
    (visual, caret_row, caret_col)
}

/// Where the caret is, in folded rows and columns, for the terminal's own cursor.
#[must_use]
pub fn caret(editor: &Editor, width: u16, badge: &str) -> (usize, usize) {
    let (_, row, col) = fold_all(editor, text_room(width, badge));
    (row, col)
}

/// Break one logical line into the visual rows it occupies, at `width`. On a word boundary where
/// there is one, and mid-word only for a word longer than the whole width.
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

/// Where the cursor lands once `text` is folded at `width`, as `(row, column)` among folded rows.
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
        // Not in front of it: typing lands on column zero whatever the box happens to be saying.
        let row = cells("build", None);
        assert_eq!(row[0], ("b".to_owned(), true), "{row:?}");
        assert_eq!(row[1], ("u".to_owned(), false), "{row:?}");
    }

    #[test]
    fn the_real_cursor_stays_put_wherever_the_other_one_is() {
        // The white block is yours; a cursor wandering off would say your text goes elsewhere.
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

/// The strip down the right of the box, and what sits in it on this row. Reserved on every row, not
/// just the badge's own: a margin that moved would reflow the right-hand edge as the prompt grew.
/// The badge sits on the middle *text* row, rounding down — a menu under the divider is not part of it.
pub(crate) fn strip(badge: &str, rows: usize, row: usize, open: bool) -> Vec<Span<'static>> {
    if badge.is_empty() {
        return Vec::new();
    }
    // A padded space each side, inverted with the name so it reads as one block, and a plain one after.
    let worn = badge.chars().count() + 3;
    if row != rows / 2 {
        return vec![Span::raw(" ".repeat(worn))];
    }
    // Reversed either way, so it is always a block; open changes only the text colour inside it.
    let ink = if open { colour::text() } else { colour::hint() };
    let mut style = Style::default().fg(ink).add_modifier(Modifier::REVERSED);
    if open {
        style = style.add_modifier(Modifier::BOLD);
    }
    vec![Span::styled(format!(" {badge} "), style), Span::raw(" ")]
}

/// The box says which session you are typing into, and the text never runs under it.
#[cfg(test)]
mod badge_tests {
    use super::*;

    const NAME: &str = "axum/main/alpha";

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
        .lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
    }

    #[test]
    fn long_text_wraps_rather_than_running_under_the_badge() {
        // A logical line drawn as one row ran the text through the right-hand border.
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
        // One row has it beside the text; a tall one has it level with the middle.
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
        // A margin only on the badge's own row would reflow the block as the prompt grew past a line.
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
        // A caret counted along a logical line is on the wrong row once that line wraps.
        let mut editor = Editor::new();
        editor.insert_str(&"b".repeat(60));
        let (row, col) = caret(&editor, 50, NAME);
        assert!(row > 0, "the caret stayed on the first row");
        assert!(col <= text_room(50, NAME), "it is off the right edge");
    }
}

/// A character you type arrives as a symbol and resolves into itself.
#[cfg(test)]
mod resolving_tests {
    use super::*;

    fn line_of(text: &str) -> String {
        let mut editor = Editor::new();
        editor.insert_str(text);
        resolving(&editor, 0)
    }

    #[test]
    fn off_is_off() {
        // Zero is the built-in, and a config that says nothing about this gets what it typed.
        assert_eq!(crate::metric::BUILT_IN.type_reveal_ms, 0);
        assert_eq!(line_of("hello"), "hello");
    }

    #[test]
    fn the_stages_are_symbols_and_end_in_the_letter() {
        // A letter passing through another letter reads as a typo correcting itself.
        let stages = crate::glyph::type_stages();
        assert!(!stages.is_empty());
        assert!(
            !stages.chars().any(char::is_alphanumeric),
            "a stage that is a letter reads as a typo: {stages:?}"
        );
    }

    #[test]
    fn a_character_that_was_not_just_typed_is_left_alone() {
        // Text recalled from history, or pasted and settled, must not flicker on every redraw.
        let mut editor = Editor::new();
        editor.insert_str("settled");
        // Nothing matches at a position holding a different character.
        assert!(editor.typed_age(0, 0, 'x').is_none());
        assert!(editor.typed_age(9, 0, 's').is_none());
    }

    #[test]
    fn the_width_never_changes() {
        // Text that changes width under a border is worse than no effect at all.
        for text in ["a", "hello world", "unicode: ✓ ✗"] {
            assert_eq!(
                line_of(text).chars().count(),
                text.chars().count(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn what_was_typed_is_remembered_where_it_was_typed() {
        let mut editor = Editor::new();
        editor.insert('h');
        editor.insert('i');
        assert!(editor.typed_age(0, 0, 'h').is_some());
        assert!(editor.typed_age(0, 1, 'i').is_some());
        assert!(
            editor.typed_age(0, 1, 'h').is_none(),
            "not by position alone"
        );
    }
}
