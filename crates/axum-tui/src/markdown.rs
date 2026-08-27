//! Markdown to styled lines.
//!
//! Covers what a transcript actually contains: headings, fenced and inline code, emphasis,
//! lists, block quotes, and rules. Pi's renderer is 1,015 lines and also does tables, links
//! with URL dimming, and LaTeX; those wait until a transcript needs them.

use crate::theme::Theme;
use crate::wrap;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Render markdown into wrapped, styled lines.
///
/// `width` is the space available for text after padding is applied by the caller.
#[must_use]
pub fn render(source: &str, width: u16, theme: &Theme, base: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_fence = false;
    // Consecutive prose lines are one paragraph and reflow together; a hard break in the
    // source is an artifact of how the model emitted it, not something the reader asked for.
    let mut paragraph: Vec<&str> = Vec::new();

    for raw in source.lines() {
        let trimmed = raw.trim_end();

        if let Some(rest) = fence_marker(trimmed) {
            flush_paragraph(&mut paragraph, &mut out, width, theme, base);
            in_fence = !in_fence;
            if in_fence && !rest.is_empty() {
                out.push(Line::from(Span::styled(
                    rest.to_owned(),
                    Style::default().fg(theme.muted),
                )));
            }
            continue;
        }

        if in_fence {
            out.push(Line::from(Span::styled(
                trimmed.to_owned(),
                Style::default().fg(theme.md_code_block),
            )));
            continue;
        }

        if is_prose(trimmed) {
            paragraph.push(trimmed);
            continue;
        }

        flush_paragraph(&mut paragraph, &mut out, width, theme, base);
        out.extend(block(trimmed, width, theme, base));
    }

    flush_paragraph(&mut paragraph, &mut out, width, theme, base);
    out
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
    theme: &Theme,
    base: Style,
) {
    if paragraph.is_empty() {
        return;
    }
    let joined = paragraph.join(" ");
    paragraph.clear();
    out.extend(block(&joined, width, theme, base));
}

/// The text after a ``` marker, or `None` if this is not a fence line.
fn fence_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed.strip_prefix("```").map(str::trim)
}

fn block(line: &str, width: u16, theme: &Theme, base: Style) -> Vec<Line<'static>> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    if trimmed.is_empty() {
        return vec![Line::default()];
    }

    if is_rule(trimmed) {
        let rule = "─".repeat(usize::from(width).max(1));
        return vec![Line::from(Span::styled(
            rule,
            Style::default().fg(theme.md_quote),
        ))];
    }

    if let Some(hashes) = heading_level(trimmed) {
        let text = trimmed[hashes..].trim_start().to_owned();
        return wrap::line(
            Line::from(Span::styled(
                text,
                Style::default()
                    .fg(theme.md_heading)
                    .add_modifier(Modifier::BOLD),
            )),
            width,
        );
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        let mut spans = vec![Span::styled("│ ", Style::default().fg(theme.md_quote))];
        spans.extend(inline(rest, theme, base.fg(theme.md_quote)));
        return wrap::line(Line::from(spans), width);
    }

    if let Some((marker, rest)) = list_marker(trimmed) {
        let mut spans = vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(marker, Style::default().fg(theme.accent)),
        ];
        spans.extend(inline(rest, theme, base));
        return wrap::line(Line::from(spans), width);
    }

    let mut spans = Vec::new();
    if indent > 0 {
        spans.push(Span::raw(" ".repeat(indent)));
    }
    spans.extend(inline(trimmed, theme, base));
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
            return Some(("• ".to_owned(), rest));
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
fn inline(text: &str, theme: &Theme, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '`' => {
                flush(&mut buf, &mut spans, base);
                let mut code = String::new();
                for c in chars.by_ref() {
                    if c == '`' {
                        break;
                    }
                    code.push(c);
                }
                spans.push(Span::styled(code, Style::default().fg(theme.md_code)));
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                flush(&mut buf, &mut spans, base);
                let mut bold = String::new();
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        break;
                    }
                    bold.push(c);
                }
                spans.push(Span::styled(bold, base.add_modifier(Modifier::BOLD)));
            }
            '*' | '_' => {
                let delim = c;
                flush(&mut buf, &mut spans, base);
                let mut italic = String::new();
                for c in chars.by_ref() {
                    if c == delim {
                        break;
                    }
                    italic.push(c);
                }
                spans.push(Span::styled(italic, base.add_modifier(Modifier::ITALIC)));
            }
            _ => buf.push(c),
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
        render(source, 40, &Theme::default(), Style::default())
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
    fn fenced_code_keeps_indentation_and_drops_the_fence() {
        assert_eq!(
            lines_of("```rust\n    let x = 1;\n```"),
            vec!["rust", "    let x = 1;"]
        );
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
