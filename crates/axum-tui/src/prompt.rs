//! The prompt, as Pi draws it.
//!
//! A horizontal rule above and below the text, nothing at the sides, and no gutter — Pi's
//! `editorPaddingX` defaults to `0`, so the text starts in column zero. The cursor is drawn
//! into the line with inverse video rather than parked with the terminal's own cursor, which
//! is what lets the block scroll and wrap without the cursor drifting off it.

use crate::editor::Editor;
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Rows the prompt shows before it scrolls, as a fraction of the terminal.
const VISIBLE_FRACTION: f32 = 0.3;

/// Rows the prompt shows at minimum, however short the terminal is.
const MIN_VISIBLE: usize = 5;

/// How many text rows the prompt will show on a terminal `rows` tall.
#[must_use]
pub fn visible_rows(rows: u16) -> usize {
    ((f32::from(rows) * VISIBLE_FRACTION) as usize).max(MIN_VISIBLE)
}

/// What an empty prompt says instead of nothing.
///
/// An empty box between two rules gives a reader no way to tell a prompt waiting for input
/// from a screen that has hung, and no way to find the command list without being told.
const PLACEHOLDER: &str = "ask anything, or / for commands";

/// The same, for a terminal too narrow to hold it.
///
/// Shortened rather than cut: `ask anything, or / for comman` is a rendering bug on the
/// screen, and the half it loses is the half that says what to press.
const PLACEHOLDER_SHORT: &str = "/ for commands";

/// The blank prompt: the cursor, then the hint, dimmed.
fn placeholder_line(width: u16, theme: &Theme) -> Line<'static> {
    let hint = if PLACEHOLDER.chars().count() < usize::from(width) {
        PLACEHOLDER
    } else if PLACEHOLDER_SHORT.chars().count() < usize::from(width) {
        PLACEHOLDER_SHORT
    } else {
        ""
    };
    Line::from(vec![
        Span::styled(
            " ",
            Style::default()
                .fg(theme.text)
                .add_modifier(Modifier::REVERSED),
        ),
        Span::styled(hint, Style::default().fg(theme.dim)),
    ])
}

/// Render the prompt: rule, text, rule.
///
/// `rows` is the terminal height, which sets how much of a long prompt is shown before the
/// rules turn into scroll indicators.
#[must_use]
pub fn render(editor: &Editor, width: u16, rows: u16, theme: &Theme) -> Vec<Line<'static>> {
    let rule = Style::default().fg(theme.border_muted);
    let text_style = Style::default().fg(theme.text);
    let (cursor_row, cursor_col) = editor.cursor();

    let max_visible = visible_rows(rows);
    let total = editor.lines().len();
    // Scrolled to keep the cursor in view, exactly as Pi's editor does.
    let offset = cursor_row.saturating_sub(max_visible.saturating_sub(1));
    let offset = offset.min(total.saturating_sub(max_visible.min(total)));
    let end = (offset + max_visible).min(total);

    let mut out = Vec::with_capacity(end - offset + 2);
    // A blank prompt is the one case where there is nothing to draw and something to say.
    if total == 1 && editor.lines()[0].is_empty() {
        return vec![
            scroll_rule(Direction::Up, 0, width, rule),
            placeholder_line(width, theme),
            scroll_rule(Direction::Down, 0, width, rule),
        ];
    }
    out.push(scroll_rule(Direction::Up, offset, width, rule));

    for (index, text) in editor.lines()[offset..end].iter().enumerate() {
        let row = offset + index;
        out.push(if row == cursor_row {
            with_cursor(text, cursor_col, text_style)
        } else {
            Line::from(Span::styled(text.clone(), text_style))
        });
    }

    let below = total.saturating_sub(end);
    out.push(scroll_rule(Direction::Down, below, width, rule));
    out
}

enum Direction {
    Up,
    Down,
}

/// A plain rule, or one that says how many lines are hidden in that direction.
fn scroll_rule(direction: Direction, hidden: usize, width: u16, style: Style) -> Line<'static> {
    let width = usize::from(width).max(1);
    if hidden == 0 {
        return Line::from(Span::styled("─".repeat(width), style));
    }

    let arrow = match direction {
        Direction::Up => '↑',
        Direction::Down => '↓',
    };
    let label = format!("─── {arrow} {hidden} more ");
    let used = label.chars().count();
    let text = if used <= width {
        label + &"─".repeat(width - used)
    } else {
        label.chars().take(width).collect()
    };
    Line::from(Span::styled(text, style))
}

/// Draw one line with the cursor cell inverted.
///
/// At the end of a line there is no character to invert, so a space is added and inverted —
/// which is why the layout reserves a column for it.
fn with_cursor(text: &str, col: usize, style: Style) -> Line<'static> {
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
    Line::from(spans)
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
        let rendered = rows_of(&render(&Editor::new(), 40, 24, &Theme::default()));
        assert_eq!(rendered.len(), 3, "still rule, text, rule");
        assert!(rendered[1].contains("/ for commands"), "{:?}", rendered[1]);
    }

    #[test]
    fn one_keystroke_replaces_the_hint_with_the_text() {
        let rendered = rows_of(&render(&editor_with("h"), 40, 24, &Theme::default()));
        assert!(!rendered[1].contains("commands"), "{:?}", rendered[1]);
        assert!(rendered[1].starts_with('h'), "{:?}", rendered[1]);
    }

    #[test]
    fn there_is_no_gutter_before_the_text() {
        let rendered = rows_of(&render(&editor_with("hello"), 20, 24, &Theme::default()));
        assert_eq!(rendered[1], "hello ", "text starts in column zero");
    }

    #[test]
    fn the_cursor_cell_is_inverted_in_place() {
        let mut editor = editor_with("abc");
        editor.home();
        let lines = render(&editor, 20, 24, &Theme::default());
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), "a");
    }

    #[test]
    fn a_cursor_at_the_end_inverts_an_added_space() {
        let lines = render(&editor_with("ab"), 20, 24, &Theme::default());
        let cursor = lines[1]
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .expect("an inverted span");
        assert_eq!(cursor.content.as_ref(), " ");
    }

    #[test]
    fn the_rules_span_the_full_width() {
        let lines = render(&editor_with("x"), 30, 24, &Theme::default());
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
        let rendered = rows_of(&render(&editor_with(&body), 40, 24, &Theme::default()));
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
        let rendered = rows_of(&render(&editor, 40, 24, &Theme::default()));
        assert_eq!(rendered.len(), visible_rows(24) + 2, "rules plus text rows");
    }

    #[test]
    fn a_short_terminal_still_shows_five_rows() {
        assert_eq!(visible_rows(10), MIN_VISIBLE);
    }
}

#[cfg(test)]
mod narrow_tests {
    use super::*;

    fn row(width: u16) -> String {
        render(&Editor::new(), width, 24, &Theme::default())[1]
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
