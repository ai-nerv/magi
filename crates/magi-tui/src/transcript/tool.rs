//! How a tool call is drawn.
//!
//! The block with a background that states its outcome, the header that says which call it is,
//! and the result underneath — previewed by default, in full on request. Separate from the
//! rest of the transcript because it is the only entry kind with an inside: a user message is
//! prose and an assistant message is prose, and this is a name, arguments, a body that may be
//! a diff, and a decision about how much of it to show.

use super::clip;
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
    result: Option<&magi_proto::ToolResult>,
    width: u16,
    detail: Detail,
) -> Vec<Line<'static>> {
    let outcome = match result {
        None => colour::tool_title(),
        Some(r) if r.is_error => colour::tool_failed(),
        Some(_) => colour::tool_ok(),
    };
    let style = Style::default().bg(colour::tool_bg());

    // The name in the outcome's own colour, on nothing. It was reversed — the outcome behind it,
    // the box in front — and against a frame that carries no fill of its own a filled chip was
    // the one solid thing on an otherwise drawn-in-line edge, reading as a sticker on the box
    // rather than as its name.
    let label = Style::default().fg(outcome).add_modifier(Modifier::BOLD);
    // The handle sits at the far end of the same edge as the name, so the row has two ends that
    // belong together.
    let handle = match detail {
        Detail::Preview => crate::glyph::expand(),
        Detail::Full => crate::glyph::collapse(),
    };
    // One step further in than the header, so the two are not one column of text under a
    // coloured word.
    //
    // What a call was given is the first row, not a second header: opening a block used to show
    // its arguments, then a rule, then the output, and for an `edit` that is the same thing twice.
    let lead = super::frame::MARGIN + STEP;
    // `MARGIN` on the right, because that is what is actually there. It subtracted `block_pad`
    // — one — and the block keeps two, so every row had a column of room that did not exist: a
    // line that filled it came out a character wider than the frame, and the `…` that said it
    // had been cut was the very thing hanging past the corner.
    let body = usize::from(width).saturating_sub(lead + super::frame::MARGIN);

    // Gathered before anything is framed, because whether there is a box at all depends on
    // whether there is anything to put in one.
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut said: Vec<Line<'static>> = Vec::new();
    // What the call was given, as the block's first row. It used to ride the top edge beside the
    // name, which made the one row that says what this block *is* also the row that says what it
    // was *asked* — and a long path there had nowhere to go but into the handle. Here it has the
    // width to be read, and the edge is left to say one thing.
    let asked = summarize(args, detail);
    if !asked.trim().is_empty() {
        said.extend(laid(
            asked.trim(),
            style.fg(colour::tool_output()),
            detail,
            width,
            body,
            lead,
        ));
    }
    // No gap of its own: one blank row before every entry is laid on in `laid_out`, where the
    // two entries that need separating are both in view. Pushed here as well it was pushed
    // twice between two calls, and not at all between a message and the block after it.
    let mut out: Vec<Line<'static>> = Vec::new();
    if let Some(result) = result {
        let output = result.output.trim_end();
        if !output.is_empty() {
            let all: Vec<&str> = output.lines().collect();
            let shown = match detail {
                Detail::Preview => all.len().min(usize::from(crate::metric::preview_lines())),
                Detail::Full => all.len(),
            };
            // **What the tool said it meant, when it said anything.** A painted result carries
            // a role per span and those resolve against magi's own palette, so a diff from
            // casper and a diff from `edit` come out the same. Without one there is only
            // `change_colour`, which reads the first character and guesses -- right for a patch
            // and wrong for a `bash` running `git log --oneline`.
            let painted = match &result.shown {
                Some(magi_proto::tooling::Shown::Painted { lines }) => Some(lines),
                // A question is not output. It is drawn by whoever can answer it, and a block
                // that rendered it as text would show a picker nobody could use.
                _ => None,
            };
            for (nth, line) in all[..shown].iter().enumerate() {
                let drawn = match painted.and_then(|lines| lines.get(nth)) {
                    Some(spans) if !result.is_error => crate::painted::line(spans, style),
                    _ => {
                        let fg = if result.is_error {
                            colour::tool_failed()
                        } else {
                            change_colour(line)
                        };
                        Line::from(Span::styled((*line).to_owned(), style.fg(fg)))
                    }
                };
                rows.extend(wrapped(drawn, style, detail, width, body, lead));
            }
            // The affordance goes on the fold, because that is where a reader is
            // looking when they wonder where the rest went.
            if all.len() > shown {
                rows.push(super::frame::inside(
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

    // **A box only when there is something to put in it.** A call that has not produced anything
    // yet — most often one stopped on a permission prompt, waiting for an answer — was drawn as
    // two edges with a gap between them: an empty frame sitting on the screen behind the very
    if rows.is_empty() {
        out.push(super::frame::lone(
            name,
            label,
            &summarize(args, detail),
            width,
        ));
        return out;
    }
    out.push(super::frame::top(name, label, Some(handle), true, width));
    // Only between two things. A block showing arguments and nothing else, or a result whose
    // call had no arguments worth a row, has one half — and a rule under the only thing in the
    // box reads as a heading for an answer that never came.
    let seam = !said.is_empty() && !rows.is_empty();
    out.push(super::frame::breath(width, style));
    out.extend(said);
    if seam {
        out.push(super::frame::rule(width, style));
    }
    out.extend(rows);
    out.push(super::frame::breath(width, style));
    // What became of the call, at the end of it. `None` while it is still running: a mark either
    // way would claim an outcome nothing has reached yet.
    out.push(super::frame::closed(
        width,
        result.map(|result| !result.is_error),
    ));
    out
}

/// The colour a line of tool output is drawn in.
///
/// `edit` reports what it changed as a unified diff, and a diff drawn in one colour is a wall
/// of text with a sign column nobody reads. Applied to every tool rather than to `edit` by
/// name: a declared tool that reports a patch gets the same treatment without the renderer
/// having to be told which tools exist.
fn change_colour(line: &str) -> Color {
    // The file and hunk headers are neither added nor removed, and colouring them as changes made
    // every diff look like it added and removed its own filename. They are not context either —
    // they are the thing that says *where* — so they get a colour of their own, and a diff reads
    // as three things rather than two and a lie.
    if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
        return colour::diff_marker();
    }
    match line.as_bytes().first() {
        Some(b'+') => colour::diff_added(),
        Some(b'-') => colour::diff_removed(),
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
/// **Only a preview cuts.** This clipped to a per-argument budget whichever way the block was
/// showing, so a long `shell` command ended in `…` and opening the block did nothing about it —
/// the text was already gone by the time anything decided how much to draw. A budget is a
/// *glance* being kept scannable; asked to show the whole thing, there is nothing to budget.
fn summarize(args: &str, detail: Detail) -> String {
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_str::<serde_json::Value>(args)
    else {
        return flatten(args);
    };
    let share = match detail {
        Detail::Preview => (usize::from(crate::metric::summary_budget()) / fields.len().max(1))
            .max(usize::from(crate::metric::argument_floor())),
        Detail::Full => usize::MAX,
    };
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
    use magi_proto::Entry;
    use magi_proto::{ToolCallId, ToolResult};

    fn edit_entry(output: &str) -> Entry {
        Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "edit".into(),
            args: r#"{"path": "a.rs"}"#.into(),
            result: Some(ToolResult {
                output: output.to_owned(),
                is_error: false,
                shown: None,
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
        assert_eq!(colour_of(&lines, "-was"), Some(colour::diff_removed()));
        assert_eq!(colour_of(&lines, "+now"), Some(colour::diff_added()));
    }

    #[test]
    fn ordinary_output_keeps_the_tool_colour() {
        let lines = entry_lines(&edit_entry("edited a.rs\n"), 40, Detail::Preview);
        assert_eq!(colour_of(&lines, "edited"), Some(colour::diff_context()));
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
        assert_eq!(colour_of(&lines, "--- a.rs"), Some(colour::diff_marker()));
        assert_eq!(colour_of(&lines, "+++ a.rs"), Some(colour::diff_marker()));
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
                shown: None,
            }),
            thought_signature: None,
        };
        let lines = entry_lines(&entry, 40, Detail::Preview);
        assert_eq!(colour_of(&lines, "-was"), Some(colour::tool_failed()));
        assert_eq!(colour_of(&lines, "+now"), Some(colour::tool_failed()));
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    #[test]
    fn a_header_shows_the_value_and_not_the_key() {
        // `read "path": "a.rs"` is three kinds of punctuation around the one thing being read.
        assert_eq!(summarize(r#"{"path": "a.rs"}"#, Detail::Preview), "a.rs");
        assert_eq!(
            summarize(r#"{"command": "ls -la"}"#, Detail::Preview),
            "ls -la"
        );
    }

    #[test]
    fn a_string_argument_loses_its_escaping() {
        // The quotes are the encoding, not the value: `"println!(\"one\");"` is a short line
        // of code wearing a costume.
        assert_eq!(
            summarize(r#"{"old": "println!(\"one\");"}"#, Detail::Preview),
            "println!(\"one\");"
        );
    }

    #[test]
    fn a_long_argument_is_elided_rather_than_shown_whole() {
        // An `edit` header that repeats both sides in full is a diff written twice, once badly
        // — and the real one is two lines below it.
        let summary = summarize(
            &format!(r#"{{"new": "{}"}}"#, "x".repeat(200)),
            Detail::Preview,
        );
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
            summarize(&format!(r#"{{"command": "{command}"}}"#), Detail::Preview),
            command
        );
    }

    #[test]
    fn three_arguments_share_it() {
        // An `edit`, where the diff two lines below says what actually changed.
        let long = "y".repeat(100);
        let summary = summarize(
            &format!(r#"{{"a": "{long}", "b": "{long}", "c": "{long}"}}"#),
            Detail::Preview,
        );
        assert!(
            summary.chars().count() <= usize::from(crate::metric::summary_budget()) + 4,
            "{summary}"
        );
        assert_eq!(summary.matches('…').count(), 3, "each was cut: {summary}");
    }

    #[test]
    fn several_arguments_are_separated_plainly() {
        let summary = summarize(r#"{"a": "one", "b": "two"}"#, Detail::Preview);
        assert_eq!(summary, "one, two");
    }

    #[test]
    fn a_multi_line_argument_stays_on_one_line() {
        // A heredoc in a `bash` call would otherwise push the whole block sideways.
        let summary = summarize("{\"command\": \"echo a\\necho b\"}", Detail::Preview);
        assert!(!summary.contains('\n'), "{summary}");
        assert_eq!(summary, "echo a echo b");
    }

    #[test]
    fn a_non_string_argument_is_still_shown() {
        assert_eq!(
            summarize(r#"{"lines": 42, "all": true}"#, Detail::Preview),
            "42, true"
        );
    }

    #[test]
    fn arguments_keep_the_order_the_model_sent_them_in() {
        // Sorted by key, an `edit` header reads `new, old, path` — the thing being edited
        // last, after both sides of a change the diff below is about to show properly.
        assert_eq!(
            summarize(
                r#"{"path": "a.rs", "old": "x", "new": "y"}"#,
                Detail::Preview
            ),
            "a.rs, x, y"
        );
    }

    #[test]
    fn arguments_that_are_not_an_object_fall_back_rather_than_vanishing() {
        // A tool may take anything, and a header that renders nothing is worse than one that
        // renders awkwardly.
        assert_eq!(
            summarize("not json at all", Detail::Preview),
            "not json at all"
        );
        assert_eq!(summarize("[1, 2]", Detail::Preview), "[1, 2]");
    }

    #[test]
    fn a_call_with_no_arguments_summarises_to_nothing() {
        assert_eq!(summarize("{}", Detail::Preview), "");
    }
}

#[cfg(test)]
mod detail_tests {
    use super::*;
    use crate::transcript::{Detail, entry_lines};
    use magi_proto::Entry;
    use magi_proto::{ToolCallId, ToolResult};

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
                shown: None,
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
        let result = magi_proto::ToolResult {
            output: body,
            is_error: false,
            shown: None,
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
    use magi_proto::ToolResult;

    fn header(result: Option<&ToolResult>) -> Line<'static> {
        block(
            "shell",
            r#"{"command":"echo hi"}"#,
            result,
            40,
            Detail::Preview,
        )
        .into_iter()
        .next()
        .expect("the header row")
    }

    #[test]
    fn nothing_on_the_top_edge_paints_a_background() {
        // The frame is the outer thing and the fill is inside it, so the edge is drawn on the
        // terminal's own background — and the chips carry their colour in the foreground, not
        // behind them. Nothing on this row has a background at all.
        for span in header(None).spans {
            assert_eq!(
                span.style.bg, None,
                "{:?} paints something behind itself on the frame",
                span.content
            );
        }
    }

    #[test]
    fn the_name_says_what_happened() {
        // The outcome is the name's *foreground* now: a filled chip on a frame drawn in line
        // characters was the one solid thing on the edge, so the colour moved to the letters.
        let ok = ToolResult {
            output: "hi".into(),
            is_error: false,
            shown: None,
        };
        let bad = ToolResult {
            is_error: true,
            ..ok.clone()
        };
        let coloured = |result: Option<&ToolResult>| {
            header(result)
                .spans
                .iter()
                .find(|s| s.content.contains("shell"))
                .expect("the name")
                .style
                .fg
        };
        assert_eq!(coloured(None), Some(colour::tool_title()), "still running");
        assert_eq!(coloured(Some(&ok)), Some(colour::tool_ok()));
        assert_eq!(coloured(Some(&bad)), Some(colour::tool_failed()));
    }
}

#[cfg(test)]
#[path = "header.rs"]
mod header_tests;

#[cfg(test)]
#[path = "handle.rs"]
mod handle_tests;

/// One line of a block's contents, cut or wrapped according to how much was asked for.
///
/// **This is what opening a block is for.** Every row was cut to the width whichever way the
/// block was showing, so a long line ended in `…` open or shut — and the key that was supposed to
/// reveal it added rows underneath without touching the one thing the reader was looking at.
/// Pressing it on a short result did nothing at all, visibly.
///
/// So: a preview cuts, because a preview is a glance and one row per line is what makes it
/// scannable. Open, nothing is hidden — a long line wraps and every character of it is there.
/// The same, for a line that is already spans.
///
/// [`laid`] builds its own line out of one string; a painted row arrives with a colour per span
/// and must keep them, so the clipping and wrapping are applied to what it already is. One
/// function doing both would either lose the spans or wrap the string twice.
fn wrapped(
    line: Line<'static>,
    style: Style,
    detail: Detail,
    width: u16,
    body: usize,
    lead: usize,
) -> Vec<Line<'static>> {
    // **Before anything measures.** A tab is one character and any number of columns, so a row
    // that still held one would be counted short and drawn long — out past the frame it was
    // just clipped to fit inside. `laid` expands on the way in for the same reason.
    let line = Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(crate::wrap::expand_tabs(&span.content), span.style))
            .collect::<Vec<_>>(),
    );
    match detail {
        // A preview cuts rather than wraps, so a long line costs one row here as it does
        // everywhere else in a folded block.
        Detail::Preview => {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let room = crate::wrap::columns(&clip(&text, body));
            let mut kept = Vec::new();
            let mut used = 0_usize;
            for span in line.spans {
                if used >= room {
                    break;
                }
                let wide = crate::wrap::columns(&span.content);
                if used + wide <= room {
                    used += wide;
                    kept.push(span);
                } else {
                    // The span that straddles the edge is cut, and keeps its colour: a row that
                    // dropped it would end early, and one that kept it whole would overrun.
                    let content = clip(&span.content, room - used);
                    used = room;
                    kept.push(Span::styled(content, span.style));
                }
            }
            vec![super::frame::inside(Line::from(kept), width, style, lead)]
        }
        Detail::Full => crate::wrap::line(line, u16::try_from(body).unwrap_or(u16::MAX))
            .into_iter()
            .map(|part| super::frame::inside(part, width, style, lead))
            .collect(),
    }
}

fn laid(
    text: &str,
    style: Style,
    detail: Detail,
    width: u16,
    body: usize,
    lead: usize,
) -> Vec<Line<'static>> {
    match detail {
        Detail::Preview => vec![super::frame::inside(
            Line::from(Span::styled(clip(text, body), style)),
            width,
            style,
            lead,
        )],
        Detail::Full => {
            let whole = Line::from(Span::styled(crate::wrap::expand_tabs(text), style));
            crate::wrap::line(whole, u16::try_from(body).unwrap_or(u16::MAX))
                .into_iter()
                .map(|part| super::frame::inside(part, width, style, lead))
                .collect()
        }
    }
}

/// Opening a block shows what a preview cut, and nothing ever leaves the frame.
#[cfg(test)]
#[path = "revealing.rs"]
mod revealing;

#[cfg(test)]
mod arguments {
    use super::*;

    const LONG: &str =
        "git log --oneline --graph --decorate --all --since='2 weeks ago' -- crates/magi-tui/src";

    fn args() -> String {
        format!(r#"{{"command": "{LONG}"}}"#)
    }

    #[test]
    fn a_preview_cuts_the_command_and_opening_shows_it_whole() {
        // The complaint this is here for. The cut happened in `summarize`, before anything had
        // decided how much to draw — so a `shell` command ended in `…` and opening the block did
        // nothing, because the text was already gone.
        let shut = summarize(&args(), Detail::Preview);
        assert!(shut.ends_with('…'), "the premise: it was cut — {shut}");

        let open = summarize(&args(), Detail::Full);
        assert!(!open.contains('…'), "still cutting when open — {open}");
        assert_eq!(open, LONG, "the whole command should be there");
    }

    #[test]
    fn the_opened_block_actually_carries_it() {
        // End to end, not just the summary: the rows a reader sees have to hold the tail.
        let entry = magi_proto::Entry::Tool {
            id: magi_proto::ToolCallId::new("t1"),
            name: "shell".into(),
            args: args(),
            result: Some(magi_proto::ToolResult {
                output: "done".into(),
                is_error: false,
                shown: None,
            }),
            thought_signature: None,
        };
        let shown: String = crate::transcript::entry_lines(&entry, 56, Detail::Full)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            shown.contains("crates/magi-tui/src"),
            "the end of the command is missing: {shown}"
        );
    }
}

/// What a tool *said it meant*, drawn rather than guessed at.
#[cfg(test)]
#[path = "painting.rs"]
mod painting;
