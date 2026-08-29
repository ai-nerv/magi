//! The prompt, as Pi draws it.
//!
//! A horizontal rule above and below the text, nothing at the sides, and no gutter — Pi's
//! `editorPaddingX` defaults to `0`, so the text starts in column zero. The cursor is drawn
//! into the line with inverse video rather than parked with the terminal's own cursor, which
//! is what lets the block scroll and wrap without the cursor drifting off it.

use crate::colour;
use crate::editor::Editor;
use crate::glyph;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// How many text rows the prompt will show on a terminal `rows` tall.
#[must_use]
pub fn visible_rows(rows: u16) -> usize {
    usize::from(crate::metric::share(rows, crate::metric::prompt_share()))
        .max(usize::from(crate::metric::prompt_min_lines()))
}

/// The blank prompt: the cursor, then the hint, dimmed.
fn placeholder_spans(width: u16) -> Vec<Span<'static>> {
    let hint = if glyph::placeholder().chars().count() < usize::from(width) {
        glyph::placeholder()
    } else if glyph::placeholder_short().chars().count() < usize::from(width) {
        glyph::placeholder_short()
    } else {
        ""
    };
    vec![
        Span::styled(
            " ",
            Style::default()
                .fg(colour::text())
                .add_modifier(Modifier::REVERSED),
        ),
        Span::styled(hint, Style::default().fg(colour::hint())),
    ]
}

/// Render the prompt as a box.
///
/// Was a rule above and a rule below, which is Pi's shape. A box says where the field is, and
/// gives the scan somewhere to run: see [`crate::border`].
///
/// `rows` is the terminal height, which sets how much of a long prompt is shown before the top
/// and bottom edges start reporting what is scrolled out of view. `tick` drives the scan and
/// `scan` says what it should be doing.
#[must_use]
pub fn render(
    editor: &Editor,
    width: u16,
    rows: u16,
    tick: usize,
    scan: crate::border::Scan,
) -> Vec<Line<'static>> {
    let text_style = Style::default().fg(colour::text());
    let (cursor_row, cursor_col) = editor.cursor();
    // Two columns of the width belong to the sides now.
    let inner_width = width.saturating_sub(2);

    let max_visible = visible_rows(rows);
    let total = editor.lines().len();
    // Scrolled to keep the cursor in view, exactly as Pi's editor does.
    let offset = cursor_row.saturating_sub(max_visible.saturating_sub(1));
    let offset = offset.min(total.saturating_sub(max_visible.min(total)));
    let end = (offset + max_visible).min(total);

    let blank = total == 1 && editor.lines()[0].is_empty();
    let shown = if blank { 1 } else { end - offset };
    let (top, bottom) = crate::border::edges(width, shown, tick, scan);

    let mut out = Vec::with_capacity(shown + 2);
    out.push(hidden(top, Direction::Up, offset));

    for row in 0..shown {
        let (left, right) = crate::border::side(width, shown, row, tick, scan);
        let body = if blank {
            placeholder_spans(inner_width)
        } else {
            let index = offset + row;
            let text = &editor.lines()[index];
            if index == cursor_row {
                with_cursor(text, cursor_col, text_style)
            } else {
                vec![Span::styled(text.clone(), text_style)]
            }
        };
        let mut spans = vec![left, Span::raw(" ")];
        spans.extend(pad(body, inner_width.saturating_sub(1)));
        spans.push(right);
        out.push(Line::from(spans));
    }

    let below = total.saturating_sub(end);
    out.push(hidden(bottom, Direction::Down, below));
    out
}

/// Pad a row out so the right-hand bar lands at the edge.
fn pad(mut spans: Vec<Span<'static>>, width: u16) -> Vec<Span<'static>> {
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let room = usize::from(width).saturating_sub(used);
    if room > 0 {
        spans.push(Span::raw(" ".repeat(room)));
    }
    spans
}

/// Write "N more" into an edge when the prompt is scrolled.
///
/// On the border rather than instead of it: the box stays a box, and the count sits in it the
/// way a caption sits in a frame.
fn hidden(edge: Line<'static>, direction: Direction, count: usize) -> Line<'static> {
    if count == 0 {
        return edge;
    }
    let arrow = match direction {
        Direction::Up => '↑',
        Direction::Down => '↓',
    };
    let label = format!(" {arrow} {count} more ");
    let width: usize = edge.spans.iter().map(|s| s.content.chars().count()).sum();
    if label.chars().count() + 4 > width {
        return edge;
    }
    // Kept from the third column, so the corner and the first stretch of border survive and the
    // caption reads as part of the frame.
    let mut spans: Vec<Span<'static>> = edge.spans.into_iter().take(3).collect();
    spans.push(Span::styled(
        label.clone(),
        Style::default().fg(colour::dim()),
    ));
    let used = 3 + label.chars().count();
    for _ in used..width - 1 {
        spans.push(Span::styled("─", Style::default().fg(colour::border())));
    }
    spans.push(Span::styled(
        "╮".to_owned(),
        Style::default().fg(colour::border()),
    ));
    let last = spans.len() - 1;
    if matches!(direction, Direction::Down) {
        spans[last] = Span::styled("╯".to_owned(), Style::default().fg(colour::border()));
    }
    Line::from(spans)
}

enum Direction {
    Up,
    Down,
}

/// Draw one line with the cursor cell inverted.
///
/// At the end of a line there is no character to invert, so a space is added and inverted —
/// which is why the layout reserves a column for it.
fn with_cursor(text: &str, col: usize, style: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let inverted = style.add_modifier(Modifier::REVERSED);

    let before: String = chars[..col].iter().collect();
    let mut spans = Vec::with_capacity(3);
    if !before.is_empty() {
        spans.push(Span::styled(before, style));
    }

    match chars.get(col) {
        Some(&c) => {
            spans.push(Span::styled(c.to_string(), inverted));
            let after: String = chars[col + 1..].iter().collect();
            if !after.is_empty() {
                spans.push(Span::styled(after, style));
            }
        }
        None => spans.push(Span::styled(" ", inverted)),
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with(text: &str) -> Editor {
        let mut e = Editor::new();
        e.insert_str(text);
        e
    }

    fn rows_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn an_empty_prompt_says_what_to_do_with_it() {
        // An empty box between two rules gives no way to tell a prompt waiting for input from
        // a screen that has hung, and no way to find the command list without being told.
        let rendered = rows_of(&render(&Editor::new(), 40, 24, 0, crate::border::Scan::Off));
        assert_eq!(rendered.len(), 3, "top edge, text, bottom edge");
        assert!(rendered[1].contains("/ for commands"), "{:?}", rendered[1]);
    }

    #[test]
    fn one_keystroke_replaces_the_hint_with_the_text() {
        let rendered = rows_of(&render(
            &editor_with("h"),
            40,
            24,
            0,
            crate::border::Scan::Off,
        ));
        assert!(!rendered[1].contains("commands"), "{:?}", rendered[1]);
        assert!(
            rendered[1].starts_with("│ h"),
            "inside the box: {:?}",
            rendered[1]
        );
    }

    #[test]
    fn the_text_sits_one_column_inside_the_box() {
        let rendered = rows_of(&render(
            &editor_with("hello"),
            20,
            24,
            0,
            crate::border::Scan::Off,
        ));
        assert_eq!(
            rendered[1], "│ hello            │",
            "one column of padding, bars at both edges"
        );
    }

    #[test]
    fn the_cursor_cell_is_inverted_in_place() {
        let mut editor = editor_with("abc");
        editor.home();
        let lines = render(&editor, 20, 24, 0, crate::border::Scan::Off);
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), "a");
    }

    #[test]
    fn a_cursor_at_the_end_inverts_an_added_space() {
        let lines = render(&editor_with("ab"), 20, 24, 0, crate::border::Scan::Off);
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), " ");
    }

    #[test]
    fn the_rules_span_the_full_width() {
        let lines = render(&editor_with("x"), 30, 24, 0, crate::border::Scan::Off);
        for index in [0, lines.len() - 1] {
            let width: usize = lines[index]
                .spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum();
            assert_eq!(width, 30);
        }
    }

    #[test]
    fn a_long_prompt_scrolls_and_the_rules_say_how_much_is_hidden() {
        let body = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rendered = rows_of(&render(
            &editor_with(&body),
            40,
            24,
            0,
            crate::border::Scan::Off,
        ));
        assert!(rendered[0].contains("↑"), "{:?}", rendered[0]);
        assert!(rendered[0].contains("more"), "{:?}", rendered[0]);
        assert_eq!(
            rendered.last().map(String::as_str).map(|s| s.contains('↓')),
            Some(false),
            "the cursor is on the last line, so nothing is hidden below"
        );
    }

    #[test]
    fn scrolling_up_reports_the_lines_below() {
        let body = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut editor = editor_with(&body);
        for _ in 0..19 {
            editor.history_prev();
        }
        editor.set_text(&body);
        let rendered = rows_of(&render(&editor, 40, 24, 0, crate::border::Scan::Off));
        assert_eq!(rendered.len(), visible_rows(24) + 2, "rules plus text rows");
    }

    #[test]
    fn a_short_terminal_still_shows_five_rows() {
        assert_eq!(
            visible_rows(10),
            usize::from(crate::metric::prompt_min_lines())
        );
    }
}

#[cfg(test)]
mod narrow_tests {
    use super::*;

    fn row(width: u16) -> String {
        render(&Editor::new(), width, 24, 0, crate::border::Scan::Off)[1]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_narrow_prompt_shortens_the_hint_rather_than_cutting_it() {
        // `ask anything, or / for comman` is a rendering bug on the screen, and the half it
        // loses is the half that says what to press.
        let line = row(20);
        assert!(line.chars().count() <= 20, "{line:?}");
        assert!(line.contains("/ for commands"), "{line:?}");
    }

    #[test]
    fn a_prompt_with_no_room_at_all_says_nothing() {
        let line = row(6);
        assert!(line.chars().count() <= 6, "{line:?}");
    }

    #[test]
    fn a_wide_prompt_keeps_the_full_hint() {
        assert!(row(80).contains("ask anything"), "{:?}", row(80));
    }
}
