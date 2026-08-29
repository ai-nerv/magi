//! Markdown to styled lines.
//!
//! Covers what a transcript actually contains: headings, fenced and inline code, emphasis,
//! lists, block quotes, and rules. Pi's renderer is 1,015 lines and also does tables, links
//! with URL dimming, and LaTeX; those wait until a transcript needs them.

use crate::colour;
use crate::glyph;
use crate::table;
use crate::wrap;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Render markdown into wrapped, styled lines.
///
/// `width` is the space available for text after padding is applied by the caller.
#[must_use]
pub fn render(source: &str, width: u16, base: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_fence = false;
    // Consecutive prose lines are one paragraph and reflow together; a hard break in the
    // source is an artifact of how the model emitted it, not something the reader asked for.
    let mut paragraph: Vec<&str> = Vec::new();
    // A table is the one construct that needs several lines in hand before anything can be
    // decided about it, so its rows are held until the block ends.
    let mut table: Vec<String> = Vec::new();

    let lines: Vec<&str> = source.lines().collect();
    for (index, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_end();

        if !table.is_empty() {
            if table::is_row(trimmed) {
                table.push(trimmed.to_owned());
                continue;
            }
            out.extend(table::render(&table, width));
            table.clear();
        }

        if let Some(rest) = fence_marker(trimmed) {
            flush_paragraph(&mut paragraph, &mut out, width, base);
            in_fence = !in_fence;
            if in_fence {
                out.push(fence_head(rest, width));
            } else {
                // Closed, so the eye has something to land on. An opening bar with no closing
                // one leaves the block looking like it ran off the end of the message.
                out.push(Line::from(Span::styled(
                    "└".to_owned(),
                    Style::default().fg(colour::rule()),
                )));
            }
            continue;
        }

        if in_fence {
            out.push(Line::from(vec![
                Span::styled(glyph::quote_rule(), Style::default().fg(colour::rule())),
                Span::styled(
                    crate::wrap::expand_tabs(trimmed),
                    Style::default().fg(colour::md_code_block()),
                ),
            ]));
            continue;
        }

        // A header alone is a line with pipes in it. What makes it a table is the `|---|`
        // under it, so nothing commits until that has been seen.
        if table::is_row(trimmed)
            && lines
                .get(index + 1)
                .is_some_and(|next| table::is_separator(next))
        {
            flush_paragraph(&mut paragraph, &mut out, width, base);
            table.push(trimmed.to_owned());
            continue;
        }

        if is_prose(trimmed) {
            paragraph.push(trimmed);
            continue;
        }

        flush_paragraph(&mut paragraph, &mut out, width, base);
        out.extend(block(trimmed, width, base));
    }

    flush_paragraph(&mut paragraph, &mut out, width, base);
    if !table.is_empty() {
        out.extend(table::render(&table, width));
    }
    out
}

/// The opening line of a fenced block: the bar, and the language if one was named.
fn fence_head(language: &str, width: u16) -> Line<'static> {
    let bar = Style::default().fg(colour::rule());
    if language.is_empty() {
        return Line::from(Span::styled("┌".to_owned(), bar));
    }
    let label = format!("┌─ {language} ");
    let used = label.chars().count();
    let rest = usize::from(width).saturating_sub(used).min(8);
    Line::from(vec![
        Span::styled(label, bar),
        Span::styled("─".repeat(rest), bar),
    ])
}

/// Whether a line is ordinary paragraph text rather than a block of its own.
fn is_prose(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && !is_rule(trimmed)
        && heading_level(trimmed).is_none()
        && !trimmed.starts_with("> ")
        && list_marker(trimmed).is_none()
}

/// Render the accumulated paragraph as one reflowed block.
fn flush_paragraph(
    paragraph: &mut Vec<&str>,
    out: &mut Vec<Line<'static>>,
    width: u16,
    base: Style,
) {
    if paragraph.is_empty() {
        return;
    }
    let joined = paragraph.join(" ");
    paragraph.clear();
    out.extend(block(&joined, width, base));
}

/// The text after a ``` marker, or `None` if this is not a fence line.
fn fence_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed.strip_prefix("```").map(str::trim)
}

fn block(line: &str, width: u16, base: Style) -> Vec<Line<'static>> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    if trimmed.is_empty() {
        return vec![Line::default()];
    }

    if is_rule(trimmed) {
        let rule = "─".repeat(usize::from(width).max(1));
        return vec![Line::from(Span::styled(
            rule,
            Style::default().fg(colour::md_quote()),
        ))];
    }

    if let Some(hashes) = heading_level(trimmed) {
        let text = trimmed[hashes..].trim_start().to_owned();
        return wrap::line(
            Line::from(Span::styled(
                text,
                Style::default()
                    .fg(colour::md_heading())
                    .add_modifier(Modifier::BOLD),
            )),
            width,
        );
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        let mut spans = vec![Span::styled(
            glyph::quote_rule(),
            Style::default().fg(colour::md_quote()),
        )];
        spans.extend(inline(rest, base.fg(colour::md_quote())));
        return wrap::line(Line::from(spans), width);
    }

    if let Some((marker, rest)) = list_marker(trimmed) {
        let mut spans = vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(marker, Style::default().fg(colour::accent())),
        ];
        spans.extend(inline(rest, base));
        return wrap::line(Line::from(spans), width);
    }

    let mut spans = Vec::new();
    if indent > 0 {
        spans.push(Span::raw(" ".repeat(indent)));
    }
    spans.extend(inline(trimmed, base));
    wrap::line(Line::from(spans), width)
}

fn is_rule(line: &str) -> bool {
    let body: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    body.len() >= 3 && (body.chars().all(|c| c == '-') || body.chars().all(|c| c == '*'))
}

fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    (1..=6).contains(&hashes).then_some(hashes)
}

/// Split a list line into its marker and the rest.
///
/// Ordered markers are preserved verbatim rather than renumbered — Pi passes
/// `preserveOrderedListMarkers` for exactly this, because a model that writes `3.` means it.
fn list_marker(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return Some((glyph::bullet().to_owned(), rest));
        }
    }

    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(after) = rest.strip_prefix(". ") {
            return Some((format!("{}. ", &line[..digits]), after));
        }
    }
    None
}

/// Emphasis and inline code within one line.
fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        // Looked up rather than tried and rolled back: a delimiter with no partner is ordinary
        // text, and deciding that after the fact means putting back everything consumed while
        // finding out — which is where the text before it went missing.
        let closes = |from: usize, delim: char, run: usize| -> Option<usize> {
            let mut j = from;
            while j + run <= chars.len() {
                if chars[j] == delim && (run == 1 || chars.get(j + 1) == Some(&delim)) {
                    // `_` is a word character as far as a name is concerned, so a closer that
                    // runs straight into one is not closing anything.
                    let inside_word =
                        delim == '_' && chars.get(j + run).is_some_and(|n| n.is_alphanumeric());
                    if !inside_word {
                        return Some(j);
                    }
                }
                j += 1;
            }
            None
        };

        match c {
            '`' => {
                if let Some(end) = chars[i + 1..].iter().position(|&n| n == '`') {
                    flush(&mut buf, &mut spans, base);
                    let code: String = chars[i + 1..i + 1 + end].iter().collect();
                    spans.push(Span::styled(code, Style::default().fg(colour::md_code())));
                    i += end + 2;
                    continue;
                }
                buf.push(c);
                i += 1;
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                if let Some(end) = closes(i + 2, '*', 2) {
                    flush(&mut buf, &mut spans, base);
                    let bold: String = chars[i + 2..end].iter().collect();
                    spans.push(Span::styled(bold, base.add_modifier(Modifier::BOLD)));
                    i = end + 2;
                    continue;
                }
                buf.push(c);
                i += 1;
            }
            // An underscore inside a word is part of the word. `ANT_LING_API_KEY` is a variable
            // name, not `ANT` and an italic `LING` and `API_KEY` — and it is the name somebody
            // has just been told to set, so eating half of it is worse than rendering no
            // emphasis at all. CommonMark draws the same line for the same reason: `*` marks
            // emphasis mid-word, `_` does not.
            '_' if i > 0 && chars[i - 1].is_alphanumeric() => {
                buf.push(c);
                i += 1;
            }
            '*' | '_' => {
                if let Some(end) = closes(i + 1, c, 1) {
                    flush(&mut buf, &mut spans, base);
                    let italic: String = chars[i + 1..end].iter().collect();
                    spans.push(Span::styled(italic, base.add_modifier(Modifier::ITALIC)));
                    i = end + 1;
                    continue;
                }
                buf.push(c);
                i += 1;
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }

    flush(&mut buf, &mut spans, base);
    spans
}

fn flush(buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style) {
    if !buf.is_empty() {
        spans.push(Span::styled(std::mem::take(buf), style));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(source: &str) -> Vec<String> {
        render(source, 40, Style::default())
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn headings_lose_their_hashes() {
        assert_eq!(lines_of("## Title"), vec!["Title"]);
    }

    #[test]
    fn bullets_become_dots() {
        assert_eq!(lines_of("- one"), vec!["• one"]);
    }

    #[test]
    fn ordered_markers_are_preserved_verbatim() {
        assert_eq!(lines_of("3. third"), vec!["3. third"]);
    }

    #[test]
    fn inline_code_keeps_its_text_and_drops_its_backticks() {
        assert_eq!(lines_of("use `cargo` now"), vec!["use cargo now"]);
    }

    #[test]
    fn bold_drops_its_markers() {
        assert_eq!(lines_of("a **b** c"), vec!["a b c"]);
    }

    #[test]
    fn fenced_code_keeps_indentation_inside_its_bar() {
        // The indentation is the code's; the bar is ours and sits outside it.
        let out = lines_of("```rust\n    let x = 1;\n```");
        assert!(out[0].contains("rust"), "{out:?}");
        assert_eq!(out[1], "│     let x = 1;", "{out:?}");
    }

    #[test]
    fn a_rule_fills_the_width() {
        assert_eq!(lines_of("---"), vec!["─".repeat(40)]);
    }

    #[test]
    fn quotes_get_a_bar() {
        assert_eq!(lines_of("> quoted"), vec!["│ quoted"]);
    }
}

#[cfg(test)]
mod emphasis_tests {
    use super::*;

    fn rendered(source: &str) -> String {
        render(source, 80, Style::default())
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn an_underscore_inside_a_word_is_part_of_the_word() {
        // Found on screen: axon told somebody to `set ANT_LING_API_KEY` and rendered
        // `ANTLINGAPIKEY`. The name it ate was the whole point of the message.
        assert_eq!(rendered("set ANT_LING_API_KEY"), "set ANT_LING_API_KEY");
        assert_eq!(rendered("OPENROUTER_API_KEY"), "OPENROUTER_API_KEY");
        assert_eq!(rendered("a_b_c_d"), "a_b_c_d");
    }

    #[test]
    fn an_underscore_that_never_closes_stays_where_it_was() {
        // Otherwise one stray underscore italicises the remainder of the line and vanishes.
        assert_eq!(rendered("_unclosed emphasis"), "_unclosed emphasis");
        assert_eq!(rendered("a * b"), "a * b");
    }

    #[test]
    fn ordinary_emphasis_still_works() {
        assert_eq!(rendered("_stressed_ and *also*"), "stressed and also");
        assert_eq!(rendered("**bold** stays"), "bold stays");
    }

    #[test]
    fn a_closing_underscore_must_not_be_inside_a_word_either() {
        // `_FOO_BAR` is not an italic `FOO` with a stray `BAR`.
        assert_eq!(rendered("_FOO_BAR"), "_FOO_BAR");
    }

    #[test]
    fn a_variable_name_inside_a_sentence_survives() {
        assert_eq!(
            rendered("which is not configured: set ANT_LING_API_KEY"),
            "which is not configured: set ANT_LING_API_KEY"
        );
    }
}

#[cfg(test)]
mod block_tests {
    use super::*;

    fn rows(source: &str) -> Vec<String> {
        render(source, 60, Style::default())
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_table_is_drawn_rather_than_reflowed() {
        // Three source lines used to arrive as one paragraph of pipes wrapped across the
        // width, which is every table a model has ever emitted.
        let out = rows("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(out.iter().any(|l| l.contains('┌')), "{out:?}");
        assert!(!out.iter().any(|l| l.contains("|---|")), "{out:?}");
    }

    #[test]
    fn pipes_without_a_separator_are_still_prose() {
        // A sentence with a pipe in it is not a table, and committing on the header alone
        // would eat one.
        let out = rows("run `a | b` to pipe them");
        assert!(!out.iter().any(|l| l.contains('┌')), "{out:?}");
    }

    #[test]
    fn a_table_at_the_very_end_is_not_lost() {
        // Nothing follows it to trigger the flush.
        let out = rows("intro\n\n| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(out.iter().any(|l| l.contains('└')), "{out:?}");
    }

    #[test]
    fn prose_after_a_table_starts_again() {
        let out = rows("| a |\n|---|\n| 1 |\nafter");
        assert!(out.iter().any(|l| l.trim() == "after"), "{out:?}");
        assert!(out.iter().any(|l| l.contains('└')), "{out:?}");
    }

    #[test]
    fn a_fenced_block_is_enclosed_rather_than_recoloured() {
        // Without a bar a code block is prose in a different colour, and the language tag
        // above it reads as a stray word rather than a label on anything.
        let out = rows("```rust\nfn main() {}\n```");
        assert!(out[0].contains("rust"), "{out:?}");
        assert!(out[0].starts_with('┌'), "{out:?}");
        assert!(out[1].starts_with('│'), "{out:?}");
    }

    #[test]
    fn a_fence_with_no_language_still_gets_its_bar() {
        let out = rows("```\nplain\n```");
        assert!(out[1].starts_with('│'), "{out:?}");
    }

    #[test]
    fn a_table_inside_a_fence_is_left_alone() {
        // Inside a fence everything is text, pipes included.
        let out = rows("```\n| a | b |\n|---|---|\n```");
        assert!(!out.iter().any(|l| l.contains('┬')), "{out:?}");
    }
}

#[cfg(test)]
mod fence_close_tests {
    use super::*;

    fn rows(source: &str) -> Vec<String> {
        render(source, 60, Style::default())
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_closed_fence_is_closed_on_the_screen_too() {
        // An opening bar with no closing one leaves the block looking like it ran off the end.
        let out = rows("```rust\nfn main() {}\n```");
        assert!(out.last().expect("a line").starts_with('└'), "{out:?}");
    }

    #[test]
    fn an_unclosed_fence_gets_no_closing_bar() {
        // A block still streaming has not ended, and drawing an end would say it had.
        let out = rows("```rust\nfn main() {");
        assert!(!out.iter().any(|l| l.starts_with('└')), "{out:?}");
    }
}
