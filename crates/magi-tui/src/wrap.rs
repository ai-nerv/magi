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
    let total: usize = source.spans.iter().map(|s| columns(&s.content)).sum();
    if total <= width {
        return vec![source];
    }

    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0_usize;

    for span in source.spans {
        let style = span.style;
        for word in split_keeping_spaces(span.content.as_ref()) {
            let len = columns(&word);

            // A leading space on a fresh row is the break itself; dropping it keeps the left
            // edge aligned instead of stepping in by one per wrapped row.
            if used == 0 && word.trim().is_empty() {
                continue;
            }

            if used + len > width {
                if len > width {
                    let mut rest = word.as_str();
                    while !rest.is_empty() {
                        // **Filled by column and advanced by byte.** It took `room` *characters*
                        // and then advanced by the *columns* they came to — two numbers that are
                        // only ever the same for text one column per character, which is the one
                        // case a hard split does not have to think about.
                        let room = width - used;
                        let mut taken = 0;
                        let mut wide = 0;
                        for (at, c) in rest.char_indices() {
                            let next = columns(c.encode_utf8(&mut [0u8; 4]));
                            if wide + next > room {
                                break;
                            }
                            wide += next;
                            taken = at + c.len_utf8();
                        }
                        // A single glyph wider than the whole row would otherwise take nothing
                        // and loop for ever.
                        if taken == 0 {
                            taken = rest.char_indices().nth(1).map_or(rest.len(), |(at, _)| at);
                        }
                        current.push(Span::styled(rest[..taken].to_owned(), style));
                        rows.push(Line::from(std::mem::take(&mut current)));
                        used = 0;
                        rest = &rest[taken..];
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
            let len = columns(&word);
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
            assert!(columns(&row) <= 12, "{row:?}");
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
            let stop = usize::from(crate::metric::tab_width())
                - (column % usize::from(crate::metric::tab_width()));
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

/// How many columns `text` occupies on a terminal.
///
/// **Not how many characters it has.** A `▸`, a CJK glyph and an emoji are each one `char` and
/// two columns wide, and everything that lays out a row — a frame's fill, a clip, a wrap — was
/// counting characters. One wide glyph in a line therefore pushed the row a column past the
/// frame, and a screen with any of them in it was ragged down the right.
///
/// Tabs are expanded first, because a tab is one character and several columns, and a zero-width
/// joiner or a combining accent is one character and none.
#[must_use]
pub fn columns(text: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(expand_tabs(text).as_str())
}

/// Break `text` into rows of at most `width` columns, at a space where there is one.
///
/// For things that are not prose — a command, a path — so it falls back to breaking mid-word
/// rather than letting a long unbroken run overrun the box it is being drawn in.
#[must_use]
pub fn hard(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim_end();
        while columns(rest) > width {
            // A space inside the budget, if there is one: breaking a command at an argument
            // boundary keeps each row readable, and breaking mid-token does not.
            let take = rest
                .char_indices()
                .map(|(at, c)| at + c.len_utf8())
                .take_while(|end| columns(&rest[..*end]) <= width)
                .last()
                .unwrap_or(rest.len());
            let cut = rest[..take].rfind(' ').map_or(take, |at| at + 1);
            rows.push(rest[..cut].trim_end().to_owned());
            rest = &rest[cut..];
        }
        rows.push(rest.to_owned());
    }
    rows
}

/// Breaking something that is not prose.
#[cfg(test)]
mod breaking {
    use super::hard;

    #[test]
    fn a_short_line_is_left_alone() {
        assert_eq!(hard("cargo test", 40), vec!["cargo test"]);
    }

    #[test]
    fn a_long_command_breaks_at_an_argument() {
        let rows = hard("cargo test --workspace --all-targets", 20);
        assert!(rows.iter().all(|row| row.chars().count() <= 20), "{rows:?}");
        assert_eq!(
            rows.concat().replace(' ', ""),
            "cargotest--workspace--all-targets"
        );
    }

    #[test]
    fn a_run_with_no_spaces_is_still_broken() {
        // A path with no break in it must not overrun the box it is drawn in.
        let long = "a".repeat(50);
        let rows = hard(&long, 20);
        assert!(rows.iter().all(|row| row.chars().count() <= 20), "{rows:?}");
        assert_eq!(rows.concat(), long);
    }
}
