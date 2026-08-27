//! Transcript entries to styled lines.
//!
//! The shape is Pi's, block for block: a user message is a full-width padded box on
//! `userMessageBg`; an assistant message is bare markdown preceded by one blank line; a tool
//! call is a padded box whose background carries its outcome.

use crate::markdown;
use crate::theme::Theme;
use axum_proto::{Entry, StopReason};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Horizontal padding inside a block, in cells. Pi's `outputPad`.
const PAD: u16 = 1;

/// Lines of a tool result shown before it is expanded. Pi's `FALLBACK_PREVIEW_LINES`.
const PREVIEW_LINES: usize = 10;

/// Render the whole transcript.
#[must_use]
pub fn render(entries: &[Entry], width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for entry in entries {
        out.extend(entry_lines(entry, width, theme));
    }
    out
}

/// Render one entry.
#[must_use]
pub fn entry_lines(entry: &Entry, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    match entry {
        Entry::User { text, .. } => user(text, width, theme),
        Entry::Assistant {
            text,
            thinking,
            stop_reason,
            error,
            ..
        } => assistant(text, thinking, *stop_reason, error.as_deref(), width, theme),
        Entry::Tool {
            name, args, result, ..
        } => tool(name, args, result.as_ref(), width, theme),
    }
}

/// A full-width box on `userMessageBg`, padded one cell on every side.
fn user(text: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let style = Style::default()
        .bg(theme.user_message_bg)
        .fg(theme.user_message_text);
    let inner = width.saturating_sub(PAD * 2);
    let body = markdown::render(text, inner, theme, style);

    let mut out = vec![blank(width, style)];
    for line in body {
        out.push(pad(line, width, style));
    }
    out.push(blank(width, style));
    out
}

/// Bare markdown with no background, preceded by one blank line.
fn assistant(
    text: &str,
    thinking: &str,
    stop_reason: Option<StopReason>,
    error: Option<&str>,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let base = Style::default().fg(theme.text);
    let inner = width.saturating_sub(PAD * 2);
    let mut out = Vec::new();

    if !thinking.trim().is_empty() || !text.trim().is_empty() {
        out.push(Line::default());
    }

    if !thinking.trim().is_empty() {
        let style = Style::default()
            .fg(theme.thinking_text)
            .add_modifier(Modifier::ITALIC);
        for line in markdown::render(thinking.trim(), inner, theme, style) {
            out.push(indent(line));
        }
        if !text.trim().is_empty() {
            out.push(Line::default());
        }
    }

    if !text.trim().is_empty() {
        for line in markdown::render(text.trim(), inner, theme, base) {
            out.push(indent(line));
        }
    }

    // A truncated response is surfaced here even when tool calls follow, because a length stop
    // can land before a call's arguments are complete and the tool block would show nothing.
    match stop_reason {
        Some(StopReason::Length) => {
            out.push(Line::default());
            out.push(indent(Line::from(Span::styled(
                "Response was truncated before completion.",
                Style::default().fg(theme.error),
            ))));
        }
        Some(StopReason::Aborted) => {
            out.push(Line::default());
            out.push(indent(Line::from(Span::styled(
                error.unwrap_or("Operation aborted").to_owned(),
                Style::default().fg(theme.error),
            ))));
        }
        Some(StopReason::Error) => {
            out.push(Line::default());
            out.push(indent(Line::from(Span::styled(
                format!("Error: {}", error.unwrap_or("Unknown error")),
                Style::default().fg(theme.error),
            ))));
        }
        _ => {}
    }

    out
}

/// A padded box whose background states the outcome: pending, success, or error.
fn tool(
    name: &str,
    args: &str,
    result: Option<&axum_proto::ToolResult>,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let bg = match result {
        None => theme.tool_pending_bg,
        Some(r) if r.is_error => theme.tool_error_bg,
        Some(_) => theme.tool_success_bg,
    };
    let style = Style::default().bg(bg);
    let inner = usize::from(width.saturating_sub(PAD * 2));

    let mut out = vec![blank(width, style), {
        let mut spans = vec![Span::styled(
            name.to_owned(),
            style.fg(theme.tool_title).add_modifier(Modifier::BOLD),
        )];
        let summary = summarize(args);
        if !summary.is_empty() {
            spans.push(Span::styled(
                format!(" {summary}"),
                style.fg(theme.tool_output),
            ));
        }
        pad(Line::from(spans), width, style)
    }];

    if let Some(result) = result {
        let body = result.output.trim_end();
        if !body.is_empty() {
            let all: Vec<&str> = body.lines().collect();
            let shown = all.len().min(PREVIEW_LINES);
            for line in &all[..shown] {
                let fg = if result.is_error {
                    theme.error
                } else {
                    change_colour(line, theme)
                };
                let clipped = clip(line, inner);
                out.push(pad(
                    Line::from(Span::styled(clipped, style.fg(fg))),
                    width,
                    style,
                ));
            }
            if all.len() > shown {
                out.push(pad(
                    Line::from(Span::styled(
                        format!("… {} more lines", all.len() - shown),
                        style.fg(theme.dim),
                    )),
                    width,
                    style,
                ));
            }
        }
    }

    out.push(blank(width, style));
    out
}

/// The colour a line of tool output is drawn in.
///
/// `edit` reports what it changed as a unified diff, and a diff drawn in one colour is a wall
/// of text with a sign column nobody reads. Applied to every tool rather than to `edit` by
/// name: a declared tool that reports a patch gets the same treatment without the renderer
/// having to be told which tools exist.
fn change_colour(line: &str, theme: &Theme) -> Color {
    match line.as_bytes().first() {
        // `+++`/`---` are file headers, not changed lines, and colouring them as changes makes
        // every diff look like it added and removed its own filename.
        Some(b'+') if !line.starts_with("+++") => theme.diff_added,
        Some(b'-') if !line.starts_with("---") => theme.diff_removed,
        _ => theme.tool_output,
    }
}

/// A one-line summary of a tool's arguments for the block header.
///
/// The full JSON belongs in an expanded view; the header shows the values, which is what
/// identifies the call at a glance.
fn summarize(args: &str) -> String {
    let flat: String = args
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(120)
        .collect();
    flat.trim_matches(|c| c == '{' || c == '}')
        .trim()
        .to_owned()
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    text.chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

/// A full-width line carrying only the background.
fn blank(width: u16, style: Style) -> Line<'static> {
    Line::from(Span::styled(" ".repeat(usize::from(width)), style))
}

/// Indent a line by [`PAD`] without a background.
fn indent(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(usize::from(PAD)))];
    spans.extend(line.spans);
    Line::from(spans)
}

/// Indent a line and extend its background to the full width.
///
/// The trailing fill is what makes a box read as a block rather than as ragged coloured text.
fn pad(line: Line<'static>, width: u16, style: Style) -> Line<'static> {
    let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    let pad = usize::from(PAD);
    let trailing = usize::from(width).saturating_sub(used + pad);

    let mut spans = vec![Span::styled(" ".repeat(pad), style)];
    spans.extend(line.spans);
    spans.push(Span::styled(" ".repeat(trailing), style));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_proto::{MessageId, ToolCallId, ToolResult};

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_user_message_is_a_padded_full_width_box() {
        let entry = Entry::User {
            id: MessageId::new("m1"),
            text: "hello".into(),
        };
        let lines = entry_lines(&entry, 20, &Theme::default());
        let rendered = text_of(&lines);
        assert_eq!(rendered.len(), 3, "blank, body, blank");
        assert_eq!(rendered[1], " hello              ");
        assert!(rendered.iter().all(|l| l.chars().count() == 20));
    }

    #[test]
    fn an_assistant_message_has_no_background_fill() {
        let entry = Entry::Assistant {
            id: MessageId::new("m2"),
            text: "sure".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
        };
        let rendered = text_of(&entry_lines(&entry, 20, &Theme::default()));
        assert_eq!(rendered, vec!["", " sure"]);
    }

    #[test]
    fn a_pending_tool_shows_its_name_and_args() {
        let entry = Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "read".into(),
            args: r#"{"path": "a.rs"}"#.into(),
            result: None,
        };
        let rendered = text_of(&entry_lines(&entry, 40, &Theme::default()));
        assert!(rendered[1].contains("read"), "{:?}", rendered[1]);
        assert!(rendered[1].contains("a.rs"), "{:?}", rendered[1]);
    }

    #[test]
    fn a_long_tool_result_is_previewed_with_a_remainder_count() {
        let output = (0..25)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let entry = Entry::Tool {
            id: ToolCallId::new("t2"),
            name: "bash".into(),
            args: "{}".into(),
            result: Some(ToolResult {
                output,
                is_error: false,
            }),
        };
        let rendered = text_of(&entry_lines(&entry, 40, &Theme::default()));
        assert!(
            rendered.iter().any(|l| l.contains("15 more lines")),
            "{rendered:?}"
        );
    }

    #[test]
    fn a_truncated_response_says_so() {
        let entry = Entry::Assistant {
            id: MessageId::new("m3"),
            text: "partial".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::Length),
            error: None,
        };
        let rendered = text_of(&entry_lines(&entry, 40, &Theme::default()));
        assert!(
            rendered.iter().any(|l| l.contains("truncated")),
            "{rendered:?}"
        );
    }

    #[test]
    fn an_errored_response_shows_the_message() {
        let entry = Entry::Assistant {
            id: MessageId::new("m4"),
            text: String::new(),
            thinking: String::new(),
            stop_reason: Some(StopReason::Error),
            error: Some("overloaded".into()),
        };
        let rendered = text_of(&entry_lines(&entry, 40, &Theme::default()));
        assert!(
            rendered.iter().any(|l| l.contains("Error: overloaded")),
            "{rendered:?}"
        );
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;
    use axum_proto::{ToolCallId, ToolResult};

    fn edit_entry(output: &str) -> Entry {
        Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "edit".into(),
            args: r#"{"path": "a.rs"}"#.into(),
            result: Some(ToolResult {
                output: output.to_owned(),
                is_error: false,
            }),
        }
    }

    /// The colour of the first span on the line whose text starts with `prefix`.
    fn colour_of(lines: &[Line<'static>], prefix: &str) -> Option<Color> {
        lines.iter().find_map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            text.trim_start()
                .starts_with(prefix)
                .then(|| line.spans.iter().find_map(|s| s.style.fg))?
        })
    }

    #[test]
    fn added_and_removed_lines_are_coloured_apart() {
        let theme = Theme::default();
        let lines = entry_lines(&edit_entry("edited a.rs\n-was\n+now\n"), 40, &theme);
        assert_eq!(colour_of(&lines, "-was"), Some(theme.diff_removed));
        assert_eq!(colour_of(&lines, "+now"), Some(theme.diff_added));
    }

    #[test]
    fn ordinary_output_keeps_the_tool_colour() {
        let theme = Theme::default();
        let lines = entry_lines(&edit_entry("edited a.rs\n"), 40, &theme);
        assert_eq!(colour_of(&lines, "edited"), Some(theme.tool_output));
    }

    #[test]
    fn file_headers_are_not_changes() {
        // `---`/`+++` name the file. Coloured as changes, every diff appears to add and remove
        // its own filename.
        let theme = Theme::default();
        let lines = entry_lines(&edit_entry("--- a.rs\n+++ a.rs\n-was\n"), 40, &theme);
        assert_eq!(colour_of(&lines, "--- a.rs"), Some(theme.tool_output));
        assert_eq!(colour_of(&lines, "+++ a.rs"), Some(theme.tool_output));
    }

    #[test]
    fn a_failed_tool_is_all_error_coloured_whatever_it_printed() {
        // A diff in a failure is still a failure; the block's meaning must not be diluted.
        let theme = Theme::default();
        let entry = Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "edit".into(),
            args: "{}".into(),
            result: Some(ToolResult {
                output: "-was\n+now\n".into(),
                is_error: true,
            }),
        };
        let lines = entry_lines(&entry, 40, &theme);
        assert_eq!(colour_of(&lines, "-was"), Some(theme.error));
        assert_eq!(colour_of(&lines, "+now"), Some(theme.error));
    }
}
