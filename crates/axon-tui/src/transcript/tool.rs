//! How a tool call is drawn.
//!
//! The block with a background that states its outcome, the header that says which call it is,
//! and the result underneath — previewed by default, in full on request. Separate from the
//! rest of the transcript because it is the only entry kind with an inside: a user message is
//! prose and an assistant message is prose, and this is a name, arguments, a body that may be
//! a diff, and a decision about how much of it to show.

use super::{blank, clip, pad_by};
use crate::colour;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// How much of a tool's output to show.
///
/// A named choice rather than a boolean parameter, because `render(entries, w, true)`
/// says nothing at the call site about what is true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Detail {
    /// The first few lines, and a count of what was left out.
    ///
    /// The default because a tool result is usually skimmed: an `ls`, a build, a test run.
    /// The whole of one is worth a keystroke, not the whole transcript's worth of scrolling.
    #[default]
    Preview,
    /// Every line.
    Full,
}

impl Detail {
    /// The other one, which is what a fold toggle means.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Preview => Self::Full,
            Self::Full => Self::Preview,
        }
    }
}

/// Columns the body sits in from the header.
///
/// A step rather than a second helping of [`crate::metric::block_pad`], which defaults to one:
/// a single column is not an indent, it is a misalignment.
const STEP: usize = 2;

/// A padded box whose title states the outcome: pending, success, or error.
///
/// The background used to say it, in three barely-different tints of the same dark. A palette
/// index cannot be tinted — there is no "this grey, but slightly green" in sixteen colours — so
/// the outcome moved to the one thing that can carry it at any palette: the name, in the colour
/// of what happened. It reads better anyway; a green block and a red block are the same shape
/// glanced at, and a green word and a red word are not.
pub(super) fn block(
    name: &str,
    args: &str,
    result: Option<&axon_proto::ToolResult>,
    width: u16,
    detail: Detail,
) -> Vec<Line<'static>> {
    let outcome = match result {
        None => colour::tool_title(),
        Some(r) if r.is_error => colour::tool_failed(),
        Some(_) => colour::tool_ok(),
    };
    let style = Style::default().bg(colour::tool_bg());

    // The name in reverse: the outcome behind it, the box's own background in front. A coloured
    // word states pending, done or failed; a coloured *label* states it at a glance, and reads as
    // the tag on a block rather than as the first word of a sentence. The spaces are part of it —
    // a reversed run tight against the text is a smudge.
    let label = Style::default()
        .bg(outcome)
        .fg(colour::tool_bg())
        .add_modifier(Modifier::BOLD);
    // The handle rides the header, right-aligned, and reversed the way the name is. In the
    // outcome's foreground it was a lone bright glyph at the far end of an empty row, reading as
    // debris rather than as the other end of the same header; as a chip it pairs with the name
    // and the row has two ends that belong together.
    let handle = match detail {
        Detail::Preview => crate::glyph::expand(),
        Detail::Full => crate::glyph::collapse(),
    };
    // A plain row first, not a grey one: this is the gap *between* blocks, and painted with the
    // block's own background it joined the previous block's bottom padding into one two-row
    // band. Three calls in a row then read as a single wall of grey with headings in it rather
    // than as three things that happened.
    // The name and the summary are set into the top edge, and the handle sits at the far end of
    // the same edge — so the row that says what this is is also the row that bounds it.
    let mut out = vec![
        blank(width, Style::default()),
        super::frame::top(
            name,
            label,
            Some(summarize(args).as_str()),
            Some(handle),
            width,
            style,
        ),
    ];

    // One step further in than the header, so the two are not one column of text under a
    // coloured word.
    //
    // What a call was *given* is not listed here. It was, briefly: opening a block showed its
    // arguments and then a rule and then the output. For an `edit` that is the same thing twice --
    // `old` and `new`, then a diff of `old` and `new` -- and for everything else it is a header
    // repeated a row below the header. The summary beside the name is what the call was given;
    // one line of it is enough, and a block that says it twice reads as a stutter.
    let lead = super::frame::LEAD + STEP;
    let body = usize::from(width).saturating_sub(lead + usize::from(crate::metric::block_pad()));

    if let Some(result) = result {
        let output = result.output.trim_end();
        if !output.is_empty() {
            let all: Vec<&str> = output.lines().collect();
            let shown = match detail {
                Detail::Preview => all.len().min(usize::from(crate::metric::preview_lines())),
                Detail::Full => all.len(),
            };
            for line in &all[..shown] {
                let fg = if result.is_error {
                    colour::tool_failed()
                } else {
                    change_colour(line)
                };
                out.push(pad_by(
                    Line::from(Span::styled(clip(line, body), style.fg(fg))),
                    width,
                    style,
                    lead,
                ));
            }
            // The affordance goes on the fold, because that is where a reader is
            // looking when they wonder where the rest went.
            if all.len() > shown {
                out.push(pad_by(
                    Line::from(Span::styled(
                        format!("… {} more lines · ctrl+o", all.len() - shown),
                        style.fg(colour::tool_fold()),
                    )),
                    width,
                    style,
                    lead,
                ));
            }
        }
    }

    out.push(super::frame::bottom(width, style));
    out
}

/// The colour a line of tool output is drawn in.
///
/// `edit` reports what it changed as a unified diff, and a diff drawn in one colour is a wall
/// of text with a sign column nobody reads. Applied to every tool rather than to `edit` by
/// name: a declared tool that reports a patch gets the same treatment without the renderer
/// having to be told which tools exist.
fn change_colour(line: &str) -> Color {
    match line.as_bytes().first() {
        // `+++`/`---` are file headers, not changed lines, and colouring them as changes makes
        // every diff look like it added and removed its own filename.
        Some(b'+') if !line.starts_with("+++") => colour::diff_added(),
        Some(b'-') if !line.starts_with("---") => colour::diff_removed(),
        _ => colour::diff_context(),
    }
}

/// A one-line summary of a tool's arguments for the block header.
///
/// The values, not the JSON. The keys are implied by the tool's name — `read` takes a path,
/// `bash` takes a command — and what is left after removing them is what a person is scanning
/// for. The escaping goes too: `"old": "println!(\"one\");"` is three kinds of punctuation
/// around one short string.
///
/// Falls back to the raw text for arguments that are not an object, because a tool may take
/// anything and a header that renders nothing is worse than one that renders awkwardly.
fn summarize(args: &str) -> String {
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_str::<serde_json::Value>(args)
    else {
        return flatten(args);
    };
    let share = (usize::from(crate::metric::summary_budget()) / fields.len().max(1))
        .max(usize::from(crate::metric::argument_floor()));
    fields
        .values()
        .map(|value| match value {
            // Rendered rather than serialised: a string argument is text a person reads, and
            // the quotes around it are the encoding rather than the value.
            serde_json::Value::String(text) => clip(&flatten(text), share),
            other => clip(&flatten(&other.to_string()), share),
        })
        .filter(|shown| !shown.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Collapse whitespace, so a multi-line argument stays on one line.
fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod diff_tests {
    use super::*;
    use crate::transcript::{Detail, entry_lines};
    use axon_proto::Entry;
    use axon_proto::{ToolCallId, ToolResult};

    fn edit_entry(output: &str) -> Entry {
        Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "edit".into(),
            args: r#"{"path": "a.rs"}"#.into(),
            result: Some(ToolResult {
                output: output.to_owned(),
                is_error: false,
            }),
            thought_signature: None,
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
        let lines = entry_lines(
            &edit_entry("edited a.rs\n-was\n+now\n"),
            40,
            Detail::Preview,
        );
        assert_eq!(colour_of(&lines, "-was"), Some(colour::error()));
        assert_eq!(colour_of(&lines, "+now"), Some(colour::success()));
    }

    #[test]
    fn ordinary_output_keeps_the_tool_colour() {
        let lines = entry_lines(&edit_entry("edited a.rs\n"), 40, Detail::Preview);
        assert_eq!(colour_of(&lines, "edited"), Some(colour::muted()));
    }

    #[test]
    fn file_headers_are_not_changes() {
        // `---`/`+++` name the file. Coloured as changes, every diff appears to add and remove
        // its own filename.
        let lines = entry_lines(
            &edit_entry("--- a.rs\n+++ a.rs\n-was\n"),
            40,
            Detail::Preview,
        );
        assert_eq!(colour_of(&lines, "--- a.rs"), Some(colour::muted()));
        assert_eq!(colour_of(&lines, "+++ a.rs"), Some(colour::muted()));
    }

    #[test]
    fn a_failed_tool_is_all_error_coloured_whatever_it_printed() {
        // A diff in a failure is still a failure; the block's meaning must not be diluted.
        let entry = Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "edit".into(),
            args: "{}".into(),
            result: Some(ToolResult {
                output: "-was\n+now\n".into(),
                is_error: true,
            }),
            thought_signature: None,
        };
        let lines = entry_lines(&entry, 40, Detail::Preview);
        assert_eq!(colour_of(&lines, "-was"), Some(colour::error()));
        assert_eq!(colour_of(&lines, "+now"), Some(colour::error()));
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    #[test]
    fn a_header_shows_the_value_and_not_the_key() {
        // `read "path": "a.rs"` is three kinds of punctuation around the one thing being read.
        assert_eq!(summarize(r#"{"path": "a.rs"}"#), "a.rs");
        assert_eq!(summarize(r#"{"command": "ls -la"}"#), "ls -la");
    }

    #[test]
    fn a_string_argument_loses_its_escaping() {
        // The quotes are the encoding, not the value: `"println!(\"one\");"` is a short line
        // of code wearing a costume.
        assert_eq!(
            summarize(r#"{"old": "println!(\"one\");"}"#),
            "println!(\"one\");"
        );
    }

    #[test]
    fn a_long_argument_is_elided_rather_than_shown_whole() {
        // An `edit` header that repeats both sides in full is a diff written twice, once badly
        // — and the real one is two lines below it.
        let summary = summarize(&format!(r#"{{"new": "{}"}}"#, "x".repeat(200)));
        assert!(
            summary.chars().count() <= usize::from(crate::metric::summary_budget()),
            "{summary}"
        );
        assert!(summary.ends_with('…'), "{summary}");
    }

    #[test]
    fn one_argument_gets_the_whole_budget() {
        // A `bash` command is the thing being done. Cutting it at a third of the line to
        // leave room for two arguments that do not exist helps nobody.
        let command = "ls -la && cat main.rs 2>&1 | head";
        assert_eq!(
            summarize(&format!(r#"{{"command": "{command}"}}"#)),
            command
        );
    }

    #[test]
    fn three_arguments_share_it() {
        // An `edit`, where the diff two lines below says what actually changed.
        let long = "y".repeat(100);
        let summary = summarize(&format!(
            r#"{{"a": "{long}", "b": "{long}", "c": "{long}"}}"#
        ));
        assert!(
            summary.chars().count() <= usize::from(crate::metric::summary_budget()) + 4,
            "{summary}"
        );
        assert_eq!(summary.matches('…').count(), 3, "each was cut: {summary}");
    }

    #[test]
    fn several_arguments_are_separated_plainly() {
        let summary = summarize(r#"{"a": "one", "b": "two"}"#);
        assert_eq!(summary, "one, two");
    }

    #[test]
    fn a_multi_line_argument_stays_on_one_line() {
        // A heredoc in a `bash` call would otherwise push the whole block sideways.
        let summary = summarize("{\"command\": \"echo a\\necho b\"}");
        assert!(!summary.contains('\n'), "{summary}");
        assert_eq!(summary, "echo a echo b");
    }

    #[test]
    fn a_non_string_argument_is_still_shown() {
        assert_eq!(summarize(r#"{"lines": 42, "all": true}"#), "42, true");
    }

    #[test]
    fn arguments_keep_the_order_the_model_sent_them_in() {
        // Sorted by key, an `edit` header reads `new, old, path` — the thing being edited
        // last, after both sides of a change the diff below is about to show properly.
        assert_eq!(
            summarize(r#"{"path": "a.rs", "old": "x", "new": "y"}"#),
            "a.rs, x, y"
        );
    }

    #[test]
    fn arguments_that_are_not_an_object_fall_back_rather_than_vanishing() {
        // A tool may take anything, and a header that renders nothing is worse than one that
        // renders awkwardly.
        assert_eq!(summarize("not json at all"), "not json at all");
        assert_eq!(summarize("[1, 2]"), "[1, 2]");
    }

    #[test]
    fn a_call_with_no_arguments_summarises_to_nothing() {
        assert_eq!(summarize("{}"), "");
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;
    use crate::transcript::{Detail, entry_lines};
    use axon_proto::Entry;
    use axon_proto::{ToolCallId, ToolResult};

    fn long_result(lines: usize) -> Entry {
        Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "bash".into(),
            args: r#"{"command": "ls"}"#.into(),
            result: Some(ToolResult {
                output: (0..lines)
                    .map(|i| format!("line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_error: false,
            }),
            thought_signature: None,
        }
    }

    fn text_of(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn a_preview_stops_and_says_how_much_it_left() {
        let shown = text_of(&entry_lines(&long_result(40), 40, Detail::Preview));
        assert!(shown.iter().any(|l| l.contains("line 9")), "{shown:?}");
        assert!(!shown.iter().any(|l| l.contains("line 10")), "{shown:?}");
        assert!(
            shown.iter().any(|l| l.contains("30 more lines")),
            "{shown:?}"
        );
    }

    #[test]
    fn asking_for_the_whole_thing_gets_the_whole_thing() {
        // The 190 lines of a real `ls` or test run were otherwise unreachable: the preview
        // cut them and nothing in the UI could ask for the rest.
        let shown = text_of(&entry_lines(&long_result(40), 40, Detail::Full));
        assert!(shown.iter().any(|l| l.contains("line 39")), "{shown:?}");
        assert!(
            !shown.iter().any(|l| l.contains("more lines")),
            "and does not still claim there is more: {shown:?}"
        );
    }

    #[test]
    fn a_short_result_is_not_truncated_either_way() {
        // These used to render identically. They no longer do: opening a call now also shows
        // what it was given, because the summary beside the name is one line and a `write`
        // opened to read the file it wrote was showing only the line count it reported.
        // What still has to hold is that a short result loses nothing in preview.
        let short = long_result(3);
        let preview = text_of(&entry_lines(&short, 40, Detail::Preview));
        let full = text_of(&entry_lines(&short, 40, Detail::Full));
        for line in ["line 0", "line 1", "line 2"] {
            assert!(preview.iter().any(|l| l.contains(line)), "{preview:?}");
            assert!(full.iter().any(|l| l.contains(line)), "{full:?}");
        }
        assert!(
            !preview.iter().any(|l| l.contains("more lines")),
            "{preview:?}"
        );
        assert!(
            full.iter().any(|l| l.contains("ls")),
            "and the header still names what it ran: {full:?}"
        );
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;

    fn folded(lines: usize) -> Vec<String> {
        let body: String = (1..=lines)
            .map(|i| format!("line {i}\n"))
            .collect::<String>();
        let result = axon_proto::ToolResult {
            output: body,
            is_error: false,
        };
        block(
            "bash",
            "{\"command\":\"seq\"}",
            Some(&result),
            60,
            Detail::Preview,
        )
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
    }

    #[test]
    fn the_fold_says_how_to_open_it() {
        // Without this the only place ctrl+o was mentioned was a notice printed after you had
        // already found it, and one notice was appended per press.
        let out = folded(40);
        assert!(out.iter().any(|l| l.contains("ctrl+o")), "{out:?}");
    }

    #[test]
    fn output_that_fits_gets_no_fold_and_no_hint() {
        let out = folded(3);
        assert!(!out.iter().any(|l| l.contains("ctrl+o")), "{out:?}");
    }
}

#[cfg(test)]
mod block_tests {
    use super::*;
    use axon_proto::ToolResult;

    fn header(result: Option<&ToolResult>) -> Line<'static> {
        block(
            "shell",
            r#"{"command":"echo hi"}"#,
            result,
            40,
            Detail::Preview,
        )
        .into_iter()
        .nth(1)
        .expect("the header row")
    }

    #[test]
    fn the_whole_header_row_carries_a_background() {
        // Without it the box is ragged coloured text rather than a block. Every span has one.
        // Two are reversed out of the outcome colour — the name at one end and the fold handle
        // at the other — and everything between them is the block's own.
        let reversed = |content: &str| {
            content.contains("shell")
                || content.contains(crate::glyph::expand())
                || content.contains(crate::glyph::collapse())
        };
        for span in header(None).spans {
            assert!(
                span.style.bg.is_some(),
                "{:?} has no background",
                span.content
            );
            if !reversed(&span.content) {
                assert_eq!(
                    span.style.bg,
                    Some(colour::tool_bg()),
                    "{:?} is not the block background",
                    span.content
                );
            }
        }
    }

    #[test]
    fn the_name_says_what_happened() {
        // The outcome moved from the name's foreground to its background when the name became
        // a reversed label, so this asks the label what colour it is standing on.
        let ok = ToolResult {
            output: "hi".into(),
            is_error: false,
        };
        let bad = ToolResult {
            is_error: true,
            ..ok.clone()
        };
        let behind = |result: Option<&ToolResult>| {
            header(result)
                .spans
                .iter()
                .find(|s| s.content.contains("shell"))
                .expect("the name")
                .style
                .bg
        };
        assert_eq!(behind(None), Some(colour::tool_title()), "still running");
        assert_eq!(behind(Some(&ok)), Some(colour::tool_ok()));
        assert_eq!(behind(Some(&bad)), Some(colour::tool_failed()));
    }
}

#[cfg(test)]
#[path = "header.rs"]
mod header_tests;

#[cfg(test)]
#[path = "handle.rs"]
mod handle_tests;
