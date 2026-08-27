//! Word wrapping across styled spans.
//!
//! Wrapping happens after styling, not before, so a bold run that straddles a break keeps its
//! style on both rows. Splitting the text first and styling per row would lose that.

use ratatui::text::{Line, Span};

/// Break a line at word boundaries to fit `width`.
///
/// A word longer than the whole width is hard-split rather than left to overflow — a URL or a
/// long path is common in a transcript and pushing it past the edge would clip it invisibly.
#[must_use]
pub fn line(source: Line<'static>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width).max(1);
    let total: usize = source.spans.iter().map(|s| s.content.chars().count()).sum();
    if total <= width {
        return vec![source];
    }

    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0_usize;

    for span in source.spans {
        let style = span.style;
        for word in split_keeping_spaces(span.content.as_ref()) {
            let len = word.chars().count();

            // A leading space on a fresh row is the break itself; dropping it keeps the left
            // edge aligned instead of stepping in by one per wrapped row.
            if used == 0 && word.trim().is_empty() {
                continue;
            }

            if used + len > width {
                if len > width {
                    let mut rest = word.as_str();
                    while !rest.is_empty() {
                        let room = width - used;
                        let take: String = rest.chars().take(room).collect();
                        let consumed = take.chars().count();
                        current.push(Span::styled(take, style));
                        rows.push(Line::from(std::mem::take(&mut current)));
                        used = 0;
                        rest = &rest[rest
                            .char_indices()
                            .nth(consumed)
                            .map_or(rest.len(), |(i, _)| i)..];
                    }
                    continue;
                }
                rows.push(close(std::mem::take(&mut current)));
                used = 0;
            }

            // The space that caused the break belongs to the break, not to the new row.
            let word = if used == 0 {
                word.trim_start().to_owned()
            } else {
                word
            };
            let len = word.chars().count();
            if len == 0 {
                continue;
            }
            current.push(Span::styled(word, style));
            used += len;
        }
    }

    if !current.is_empty() {
        rows.push(close(current));
    }
    rows
}

/// Finish a row, dropping the trailing space left by the word that broke it.
fn close(mut spans: Vec<Span<'static>>) -> Line<'static> {
    if let Some(last) = spans.last_mut() {
        let trimmed = last.content.trim_end().to_owned();
        if trimmed.is_empty() {
            spans.pop();
        } else {
            *last = Span::styled(trimmed, last.style);
        }
    }
    Line::from(spans)
}

/// Split into words, each carrying the whitespace that preceded it.
fn split_keeping_spaces(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_space = false;

    for c in text.chars() {
        let is_space = c == ' ' || c == '\t';
        if is_space && !in_space && !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
        }
        if !is_space && in_space {
            out.push(std::mem::take(&mut buf));
        }
        in_space = is_space;
        buf.push(c);
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Modifier, Style};

    fn rows(line: Line<'static>, width: u16) -> Vec<String> {
        super::line(line, width)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_short_line_is_untouched() {
        assert_eq!(rows(Line::from("hello"), 20), vec!["hello"]);
    }

    #[test]
    fn wrapping_breaks_between_words() {
        assert_eq!(
            rows(Line::from("the quick brown fox"), 10),
            vec!["the quick", "brown fox"]
        );
    }

    #[test]
    fn a_wrapped_row_does_not_start_with_the_space_that_broke_it() {
        for row in rows(Line::from("alpha bravo charlie delta echo"), 12) {
            assert!(!row.starts_with(' '), "{row:?}");
        }
    }

    #[test]
    fn no_row_exceeds_the_width() {
        for row in rows(Line::from("alpha beta gamma delta epsilon"), 12) {
            assert!(row.chars().count() <= 12, "{row:?}");
        }
    }

    #[test]
    fn a_word_longer_than_the_width_is_hard_split() {
        assert_eq!(
            rows(Line::from("supercalifragilistic"), 8),
            vec!["supercal", "ifragili", "stic"]
        );
    }

    #[test]
    fn a_style_survives_a_break_inside_its_span() {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let wrapped = super::line(Line::from(Span::styled("aaa bbb ccc ddd", bold)), 7);
        assert!(wrapped.len() > 1);
        for row in &wrapped {
            for span in &row.spans {
                assert!(span.style.add_modifier.contains(Modifier::BOLD));
            }
        }
    }

    #[test]
    fn styles_from_separate_spans_stay_separate() {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let wrapped = super::line(
            Line::from(vec![
                Span::styled("plain text here ", Style::default()),
                Span::styled("bold text here", bold),
            ]),
            10,
        );
        let has_bold = wrapped
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        let has_plain = wrapped
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| !s.style.add_modifier.contains(Modifier::BOLD));
        assert!(has_bold && has_plain);
    }
}

/// Columns a tab advances to. Four, because tool output is mostly code and eight wastes a
/// terminal that is already sharing its width with a padded block.
const TAB: usize = 4;

/// Replace tabs with the spaces they stand for.
///
/// A `\t` written into a cell breaks the renderer's arithmetic: the buffer counts it as one
/// column and the terminal advances the cursor to the next tab stop, so everything after it on
/// the line lands somewhere the differ does not believe it is — and the leftovers from the
/// previous frame stay on screen. Tool output is full of tabs (`read` numbers lines with one,
/// Makefiles and Go are indented with them), so this is not an edge case.
///
/// Expanded here rather than in the tools: the journal should hold what the tool actually
/// said, and only the thing painting cells needs the columns to line up.
#[must_use]
pub fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut column = 0;
    for c in text.chars() {
        if c == '\t' {
            let stop = TAB - (column % TAB);
            out.extend(std::iter::repeat_n(' ', stop));
            column += stop;
        } else {
            out.push(c);
            column += 1;
        }
    }
    out
}

#[cfg(test)]
mod tab_tests {
    use super::*;

    #[test]
    fn a_tab_becomes_spaces_to_the_next_stop() {
        assert_eq!(expand_tabs("a\tb"), "a   b");
        assert_eq!(expand_tabs("abc\td"), "abc d");
    }

    #[test]
    fn a_tab_on_a_stop_still_advances() {
        // Otherwise two adjacent columns collapse and the text after them shifts left.
        assert_eq!(expand_tabs("abcd\te"), "abcd    e");
    }

    #[test]
    fn text_without_tabs_is_returned_as_it_was() {
        assert_eq!(expand_tabs("     3}"), "     3}");
    }

    #[test]
    fn the_line_read_actually_produces_expands() {
        // `read` numbers with a tab, which is what put a stray character on screen against a
        // real model: the buffer counted one column and the terminal moved eight.
        assert_eq!(expand_tabs("     3\t}"), "     3  }");
        assert!(!expand_tabs("     3\t}").contains('\t'));
    }
}
