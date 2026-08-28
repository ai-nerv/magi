//! Markdown tables.
//!
//! The renderer treated a table's rows as prose and reflowed them into one paragraph, so
//! `| a | b |` over three lines arrived as `| a | b | |---|---| | 1 | 2 |` wrapped across the
//! width. Models emit tables constantly — a comparison, a list of flags, a summary of what a
//! change touched — and every one of them was unreadable.
//!
//! Its own module because the markdown renderer is a line-at-a-time loop and a table is the
//! one construct that needs several lines in hand before it can decide anything.

use crate::colour;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Whether a line could be part of a table.
///
/// Deliberately loose: a single pipe-delimited line is not a table, and the caller only
/// commits once it has found a separator row under a header.
#[must_use]
pub fn is_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.len() > 1
}

/// Whether a line is the `|---|:--:|` rule that makes the line above it a header.
#[must_use]
pub fn is_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !is_row(trimmed) {
        return false;
    }
    cells(trimmed).iter().all(|cell| {
        !cell.is_empty()
            && cell
                .chars()
                .all(|c| c == '-' || c == ':' || c.is_whitespace())
            && cell.contains('-')
    })
}

/// Split one row into its cells, dropping the delimiters at each end.
#[must_use]
pub fn cells(line: &str) -> Vec<String> {
    let trimmed = line.trim().trim_start_matches('|').trim_end_matches('|');
    trimmed.split('|').map(|c| c.trim().to_owned()).collect()
}

/// Render a table from its rows, the second of which is the separator.
///
/// Columns are sized to their widest cell and then shrunk proportionally if the total will not
/// fit. Cells are not wrapped: a table whose rows each take three lines has stopped being a
/// table, and the reader is better served by a truncation they can see than a shape they
/// cannot.
#[must_use]
pub fn render(rows: &[String], width: u16) -> Vec<Line<'static>> {
    let parsed: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, r)| cells(r))
        .collect();
    let Some(header) = parsed.first() else {
        return Vec::new();
    };
    let columns = parsed.iter().map(Vec::len).max().unwrap_or(0).max(1);

    let mut widths = vec![0usize; columns];
    for row in &parsed {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    fit(&mut widths, usize::from(width));

    let rule = Style::default().fg(colour::rule());
    let mut out = vec![divider(&widths, '┌', '┬', '┐', rule)];
    out.push(row_line(
        header,
        &widths,
        Style::default()
            .fg(colour::md_heading())
            .add_modifier(Modifier::BOLD),
        rule,
    ));
    out.push(divider(&widths, '├', '┼', '┤', rule));
    for row in parsed.iter().skip(1) {
        out.push(row_line(
            row,
            &widths,
            Style::default().fg(colour::text()),
            rule,
        ));
    }
    out.push(divider(&widths, '└', '┴', '┘', rule));
    out
}

/// Shrink columns until the table fits, widest first.
///
/// Widest first because a column of one-word cells has nothing to give and a column of
/// sentences has plenty; taking evenly makes the narrow ones unreadable to spare the wide one.
fn fit(widths: &mut [usize], width: usize) {
    let chrome = widths.len() * 3 + 1;
    let mut total: usize = widths.iter().sum::<usize>() + chrome;
    while total > width {
        let Some(widest) = widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > usize::from(crate::metric::min_column()))
            .max_by_key(|(_, w)| **w)
            .map(|(i, _)| i)
        else {
            break;
        };
        widths[widest] -= 1;
        total -= 1;
    }
}

fn divider(widths: &[usize], left: char, middle: char, right: char, style: Style) -> Line<'static> {
    let mut text = String::from(left);
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            text.push(middle);
        }
        text.push_str(&"─".repeat(w + 2));
    }
    text.push(right);
    Line::from(Span::styled(text, style))
}

fn row_line(row: &[String], widths: &[usize], cell_style: Style, rule: Style) -> Line<'static> {
    let mut spans = vec![Span::styled("│", rule)];
    for (i, w) in widths.iter().enumerate() {
        let cell = row.get(i).map_or("", String::as_str);
        spans.push(Span::styled(format!(" {} ", pad(cell, *w)), cell_style));
        spans.push(Span::styled("│", rule));
    }
    Line::from(spans)
}

/// A cell at exactly `width` columns: padded, or cut with an ellipsis that says it was cut.
fn pad(cell: &str, width: usize) -> String {
    let len = cell.chars().count();
    if len <= width {
        return format!("{cell}{}", " ".repeat(width - len));
    }
    let kept: String = cell.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn sample() -> Vec<String> {
        vec![
            "| Concept | Description |".to_owned(),
            "|---------|-------------|".to_owned(),
            "| fn | Defines a function |".to_owned(),
            "| println! | Prints to stdout |".to_owned(),
        ]
    }

    #[test]
    fn a_separator_row_is_recognised() {
        assert!(is_separator("|---------|-------------|"));
        assert!(is_separator("| :--- | ---: | :---: |"));
        assert!(!is_separator("| fn | Defines a function |"));
        assert!(!is_separator("| | |"));
    }

    #[test]
    fn the_separator_is_not_drawn_as_a_row() {
        // It is punctuation in the source, not data.
        let out = text_of(&render(&sample(), 60));
        assert!(!out.iter().any(|l| l.contains("-----")), "{out:?}");
    }

    #[test]
    fn every_row_gets_its_own_line() {
        // The bug: three source lines reflowed into one paragraph of pipes.
        let out = text_of(&render(&sample(), 60));
        assert!(out.iter().any(|l| l.contains("Concept")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("println!")), "{out:?}");
        assert!(
            !out.iter()
                .any(|l| l.contains("Concept") && l.contains("fn ")),
            "rows are not joined: {out:?}"
        );
    }

    #[test]
    fn columns_line_up() {
        let out = text_of(&render(&sample(), 60));
        let widths: Vec<usize> = out.iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "every line is the same width: {widths:?}"
        );
    }

    #[test]
    fn a_narrow_terminal_shrinks_the_widest_column() {
        // Taking evenly makes the narrow columns unreadable to spare the wide one.
        let out = text_of(&render(&sample(), 30));
        for line in &out {
            assert!(line.chars().count() <= 30, "{line:?} in {out:?}");
        }
        assert!(
            out.iter().any(|l| l.contains("…")),
            "the cut is visible: {out:?}"
        );
    }

    #[test]
    fn a_ragged_table_does_not_panic() {
        // Models emit rows with a missing trailing cell all the time.
        let rows = vec![
            "| a | b | c |".to_owned(),
            "|---|---|---|".to_owned(),
            "| 1 |".to_owned(),
        ];
        let out = text_of(&render(&rows, 40));
        assert!(out.len() >= 4, "{out:?}");
    }

    #[test]
    fn an_empty_table_renders_nothing() {
        assert!(render(&[], 40).is_empty());
    }
}
