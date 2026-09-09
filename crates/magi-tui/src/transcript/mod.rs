//! Transcript entries to styled lines, in Pi's shapes: a user message is a full-width padded box on
//! `userMessageBg`, an assistant message is bare markdown after one blank line, a tool call is a
//! padded box whose background carries its outcome.

use crate::colour;
use crate::glyph;
use crate::markdown;
use magi_proto::{Entry, StopReason, ToolCallId};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;

mod frame;
mod hover;
mod tool;

use frame::{MARGIN, bottom, inside, top};
use tool::STEP;

pub use hover::hovered;
pub use tool::Detail;

#[must_use]
pub fn render(entries: &[Entry], width: u16, detail: Detail) -> Vec<Line<'static>> {
    laid_out(entries, width, detail, &BTreeSet::new()).lines
}

/// A rendered transcript, and which tool call each line came from. The mapping is produced by the
/// same pass that produces the lines, so the two cannot disagree.
pub struct Laid {
    pub lines: Vec<Line<'static>>,
    /// For each line, the tool call it belongs to, if any.
    pub owners: Vec<Option<ToolCallId>>,
    /// For each line, which entry drew it. Gap rows between entries belong to neither and are `None`.
    pub blocks: Vec<Option<usize>>,
}

/// Render the whole transcript, recording which block owns each line. `flipped` names the tool calls
/// showing the opposite of `detail`, so the global fold key still moves every block not clicked.
#[must_use]
pub fn laid_out(
    entries: &[Entry],
    width: u16,
    detail: Detail,
    flipped: &BTreeSet<ToolCallId>,
) -> Laid {
    let mut laid = Laid {
        lines: Vec::new(),
        owners: Vec::new(),
        blocks: Vec::new(),
    };
    for (nth, entry) in entries.iter().enumerate() {
        let id = match entry {
            Entry::Tool { id, .. } => Some(id.clone()),
            _ => None,
        };
        let shown = match &id {
            Some(id) if flipped.contains(id) => detail.other(),
            _ => detail,
        };
        let lines = entry_lines(entry, width, shown);
        // One blank row before every entry and never two, decided here because the gap is a fact
        // about two entries meeting. Nothing before the first.
        if !laid.lines.is_empty() && !blank_row(laid.lines.last()) && !blank_row(lines.first()) {
            laid.lines.push(Line::default());
            laid.owners.push(None);
            laid.blocks.push(None);
        }
        laid.owners.extend(std::iter::repeat_n(id, lines.len()));
        laid.blocks
            .extend(std::iter::repeat_n(Some(nth), lines.len()));
        laid.lines.extend(lines);
    }
    laid
}

/// Whether a row carries nothing but space, by what it says rather than how it is styled: a tool
/// block's gap is a run of spaces and an assistant message's is an empty `Line`.
fn blank_row(line: Option<&Line<'static>>) -> bool {
    line.is_none_or(|line| {
        line.spans
            .iter()
            .all(|span| span.content.chars().all(char::is_whitespace))
    })
}

#[must_use]
pub fn entry_lines(entry: &Entry, width: u16, detail: Detail) -> Vec<Line<'static>> {
    match entry {
        Entry::User { text, .. } => user(text, width),
        Entry::Assistant {
            text,
            thinking,
            stop_reason,
            error,
            ..
        } => assistant(text, thinking, *stop_reason, error.as_deref(), width),
        Entry::Tool {
            name, args, result, ..
        } => tool::block(name, args, result.as_ref(), width, detail),
        Entry::Notice { text } => notice(text, width),
        Entry::From {
            who,
            kin,
            sort,
            text,
        } => from(who, kin, sort, text, width),
        Entry::Compaction { replaces, .. } => {
            marker(&format!(" {replaces} earlier messages summarised "), width)
        }
        // Nothing on screen. A mask changes what the provider is sent; the transcript still
        // holds what the tool said, and a rule per masked result would be a screen of rules.
        Entry::Masked { .. } => Vec::new(),
        // `keeps` is a journal index. What matters is that everything above the rule is still
        // on screen and no longer sent.
        Entry::Branch { keeps, .. } => marker(
            &if *keeps == 0 {
                " rewound — nothing above is sent from here ".to_owned()
            } else {
                format!(" rewound — only the first {keeps} messages are sent from here ")
            },
            width,
        ),
    }
}

/// Something magi is saying, marked so it cannot be read as the model saying it: a bar down the left
/// and muted text, the shape of a block quote.
fn notice(text: &str, width: u16) -> Vec<Line<'static>> {
    let style = Style::default().fg(colour::dim());
    let inner = frame::held(width);
    let mut out = vec![Line::default()];
    for line in markdown::render(text, inner, style) {
        let mut spans = vec![Span::styled(
            glyph::notice_rule(),
            Style::default().fg(colour::muted()),
        )];
        spans.extend(line.spans);
        out.push(indent(Line::from(spans)));
    }
    out
}

/// A labelled rule across the transcript. Shown rather than hidden: what the model can see above one
/// of these has changed, and this line is the answer to why it forgot something.
fn marker(label: &str, width: u16) -> Vec<Line<'static>> {
    let label = label.to_owned();
    let rule = usize::from(width).saturating_sub(crate::wrap::columns(&label));
    vec![
        Line::default(),
        Line::from(vec![
            Span::styled("─".repeat(rule / 2), Style::default().fg(colour::muted())),
            Span::styled(label, Style::default().fg(colour::dim())),
            Span::styled(
                "─".repeat(rule.saturating_sub(rule / 2)),
                Style::default().fg(colour::muted()),
            ),
        ]),
    ]
}

/// A framed full-width block, labelled `USER` in its top edge.
fn user(text: &str, width: u16) -> Vec<Line<'static>> {
    said("USER", colour::said_by_you(), text, width)
}

/// The same box, labelled with who sent it and how they stand to this session. `PARENT::alpha-rho`
/// says who and what they are to you, which is what decides whether it can be ignored.
fn from(who: &str, kin: &str, sort: &str, text: &str, width: u16) -> Vec<Line<'static>> {
    // The id alone: the project is this session's own, so printing it repeats a word every message.
    let id = who.rsplit('/').next().unwrap_or(who);
    let label = format!("{}::{id}", kin.to_uppercase());
    // The sort only when it is not the ordinary one, and into the name rather than behind it.
    let label = if sort != "note" && !sort.is_empty() {
        format!("{label} · {sort}")
    } else {
        label
    };
    said(&label, colour::said_by_agent(), text, width)
}

/// A framed block with its tag set into the top edge rather than on a row of its own.
fn said(label: &str, tag: ratatui::style::Color, text: &str, width: u16) -> Vec<Line<'static>> {
    let style = Style::default()
        .bg(colour::message_bg())
        .fg(colour::message_text());
    // Dark chip, coloured text — the other way round from a tool block, which is the one that folds.
    // In the tag's own colour on nothing: a filled chip on an unfilled frame reads as a sticker.
    let chip = Style::default().fg(tag).add_modifier(Modifier::BOLD);
    // A step in from the fill on both sides, or the box reads as a highlight behind the words.
    let lead = MARGIN + STEP;
    let inner = frame::held(width).saturating_sub(u16::try_from(STEP * 2).unwrap_or(4));
    let body = markdown::render(text, inner, style);

    // No handle: neither of these folds.
    let mut out = vec![
        top(label, chip, None, None, true, width),
        frame::breath(width, style),
    ];
    for line in body {
        out.push(inside(line, width, style, lead));
    }
    out.push(frame::breath(width, style));
    out.push(bottom(width));
    out
}

/// Bare markdown with no background, preceded by one blank line.
fn assistant(
    text: &str,
    thinking: &str,
    stop_reason: Option<StopReason>,
    error: Option<&str>,
    width: u16,
) -> Vec<Line<'static>> {
    let base = Style::default().fg(colour::text());
    let inner = frame::held(width);
    let mut out = Vec::new();

    if !thinking.trim().is_empty() || !text.trim().is_empty() {
        out.push(Line::default());
    }

    if !thinking.trim().is_empty() {
        let style = Style::default()
            .fg(colour::thinking())
            .add_modifier(Modifier::ITALIC);
        for line in markdown::render(thinking.trim(), inner, style) {
            out.push(indent(line));
        }
        if !text.trim().is_empty() {
            out.push(Line::default());
        }
    }

    // Rails around the answer and only the answer, because a copy chip has to sit in an edge and
    // thinking is not the answer. No fill, or the transcript is a stack of coloured slabs.
    if !text.trim().is_empty() {
        // A step in from the rails, so an answer and the question above it begin in one column.
        let held = frame::held(width).saturating_sub(u16::try_from(STEP * 2).unwrap_or(4));
        out.push(frame::top("", base, None, None, true, width));
        for line in markdown::render(text.trim(), held, base) {
            out.push(indent(step(line)));
        }
        out.push(bottom(width));
    }

    // Surfaced here even when tool calls follow: a length stop can land before a call's arguments
    // are complete, and the tool block would show nothing.
    match stop_reason {
        // A limit, not a fault: the model did what it could within the budget it was given.
        Some(StopReason::Length) => {
            out.push(Line::default());
            out.push(indent(Line::from(Span::styled(
                "Response hit the length limit and stopped here.",
                Style::default().fg(colour::warning()),
            ))));
        }
        // Not an error: you pressed escape and it obeyed.
        Some(StopReason::Aborted) => {
            out.push(Line::default());
            out.push(indent(Line::from(Span::styled(
                error.map_or_else(|| "Interrupted.".to_owned(), ToOwned::to_owned),
                Style::default().fg(colour::dim()),
            ))));
        }
        Some(StopReason::Error) => {
            out.push(Line::default());
            out.push(indent(Line::from(Span::styled(
                format!("Error: {}", error.unwrap_or("Unknown error")),
                Style::default().fg(colour::error()),
            ))));
        }
        _ => {}
    }

    out
}

fn clip(text: &str, width: usize) -> String {
    // Expanded before it is measured, because a tab is one character and several columns.
    let text = crate::wrap::expand_tabs(text);
    if crate::wrap::columns(&text) <= width {
        return text;
    }
    // Taken by column, not by character: cutting at `width - 1` characters is wider than asked the
    // moment one is two columns. A wide glyph that will not fit the last column is dropped.
    let room = width.saturating_sub(crate::wrap::columns(glyph::ellipsis()));
    let mut out = String::with_capacity(text.len());
    let mut used = 0;
    for c in text.chars() {
        let wide = crate::wrap::columns(c.encode_utf8(&mut [0u8; 4]));
        if used + wide > room {
            break;
        }
        used += wide;
        out.push(c);
    }
    out + glyph::ellipsis()
}

/// Put a line where a block's inside would be, so prose and blocks share one text column each side.
fn indent(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(frame::MARGIN))];
    spans.extend(line.spans);
    Line::from(spans)
}

/// One more step in, for prose that sits inside rails of its own.
fn step(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(STEP))];
    spans.extend(line.spans);
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::{MessageId, ToolCallId, ToolResult};

    pub(super) fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn from(who: &str, kin: &str, sort: &str) -> Entry {
        Entry::From {
            who: who.into(),
            kin: kin.into(),
            sort: sort.into(),
            text: "the parser is done".into(),
        }
    }

    #[test]
    fn a_user_message_is_a_framed_full_width_box() {
        let entry = Entry::User {
            id: MessageId::new("m1"),
            text: "hello".into(),
            aside: String::new(),
        };
        let lines = entry_lines(&entry, 20, Detail::Preview);
        let rendered = text_of(&lines);
        assert_eq!(
            rendered.len(),
            5,
            "an edge, a row of fill, the body, a row of fill, an edge"
        );
        // Inside the frame, not under the corner, and not pressed against the edge above it.
        assert_eq!(rendered[2], "    hello           ");
        assert!(rendered[0].starts_with('┌') && rendered[0].ends_with('┐'));
        assert!(rendered[4].starts_with('└') && rendered[4].ends_with('┘'));
        // No sides: two columns of every row taken off the text where they are least affordable.
        assert!(!rendered[2].contains('│'), "{rendered:?}");
        assert!(rendered.iter().all(|l| l.chars().count() == 20));
    }

    #[test]
    fn a_user_message_is_tagged_and_costs_no_extra_row_for_it() {
        // The tag rides the padding row rather than growing a line per message.
        let entry = Entry::User {
            id: MessageId::new("m1"),
            text: "hello".into(),
            aside: String::new(),
        };
        let rendered = text_of(&entry_lines(&entry, 20, Detail::Preview));
        assert!(rendered[0].contains("[ USER ]"), "{rendered:?}");
        // Five: the two edges, a row of fill inside each, and the text. The tag is not one of them.
        assert_eq!(rendered.len(), 5, "the tag grew a row: {rendered:?}");
        assert!(
            rendered[0].starts_with('┌'),
            "the tag rides the top edge: {rendered:?}"
        );
    }

    #[test]
    fn a_message_from_another_magi_is_tagged_with_who_and_what_they_are() {
        // The two things worth knowing, in one chip.
        let rendered = text_of(&entry_lines(
            &from("magi/alpha-rho", "parent", "note"),
            40,
            Detail::Preview,
        ));
        assert!(
            rendered[0].contains("[ PARENT::alpha-rho ]"),
            "{rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|row| row.contains("the parser is done")),
            "{rendered:?}"
        );
    }

    #[test]
    fn the_project_is_not_repeated_down_the_left_of_every_message() {
        // Nothing outside this session's own project can reach it, so the project is not printed.
        let rendered = text_of(&entry_lines(
            &from("magi/alpha-rho", "child", "note"),
            40,
            Detail::Preview,
        ));
        assert!(!rendered[0].contains("magi/"), "{rendered:?}");
    }

    #[test]
    fn an_ordinary_note_says_nothing_about_its_sort_and_an_urgent_one_does() {
        // `note` beside every message is noise. `attention` beside one is the point of sorts.
        let plain = text_of(&entry_lines(
            &from("magi/alpha-rho", "main", "note"),
            40,
            Detail::Preview,
        ));
        assert!(!plain[0].contains("note"), "{plain:?}");
        let urgent = text_of(&entry_lines(
            &from("magi/alpha-rho", "main", "attention"),
            40,
            Detail::Preview,
        ));
        assert!(urgent[0].contains("attention"), "{urgent:?}");
    }

    #[test]
    fn a_message_from_elsewhere_is_still_drawn_rather_than_dropped() {
        // A relation that makes no sense is a bug in the sender, not a reason to swallow a message.
        let rendered = text_of(&entry_lines(
            &from("other/beta-nu", "elsewhere", ""),
            40,
            Detail::Preview,
        ));
        assert!(
            rendered
                .iter()
                .any(|row| row.contains("the parser is done")),
            "{rendered:?}"
        );
    }

    #[test]
    fn an_assistant_message_has_no_background_fill() {
        let entry = Entry::Assistant {
            id: MessageId::new("m2"),
            text: "sure".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        };
        let lines = entry_lines(&entry, 20, Detail::Preview);
        // Rails, but no fill: the edge is there to carry a copy chip.
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style.bg.is_none()),
            "{lines:#?}"
        );
        // A step in from the rails, so an answer and the question above it begin in one column.
        assert!(
            text_of(&lines).contains(&"    sure".to_owned()),
            "{lines:#?}"
        );
    }

    #[test]
    fn a_pending_tool_shows_its_name_and_args() {
        let entry = Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "read".into(),
            args: r#"{"path": "a.rs"}"#.into(),
            result: None,
            thought_signature: None,
        };
        let rendered = text_of(&entry_lines(&entry, 40, Detail::Preview));
        assert!(rendered[0].contains("read"), "{:?}", rendered[0]);
        assert!(rendered[0].contains("a.rs"), "{:?}", rendered[0]);
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
                shown: None,
            }),
            thought_signature: None,
        };
        let rendered = text_of(&entry_lines(&entry, 40, Detail::Preview));
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
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        };
        let rendered = text_of(&entry_lines(&entry, 40, Detail::Preview));
        assert!(
            rendered.iter().any(|l| l.contains("length limit")),
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
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        };
        let rendered = text_of(&entry_lines(&entry, 40, Detail::Preview));
        assert!(
            rendered.iter().any(|l| l.contains("Error: overloaded")),
            "{rendered:?}"
        );
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;
    use magi_proto::{ToolCallId, ToolResult};

    /// Every character the renderer would put in a cell.
    fn cells(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect()
    }

    #[test]
    fn no_tab_reaches_the_buffer_from_tool_output() {
        // `read` numbers lines with a tab; the buffer counted it as one column and the terminal
        // moved to the next tab stop, leaving a character from the previous frame in the gap.
        let entry = Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "read".into(),
            args: "{}".into(),
            result: Some(ToolResult {
                output: "     1\tfn main() {\n     3\t}\n".into(),
                is_error: false,
                shown: None,
            }),
            thought_signature: None,
        };
        let rendered = cells(&entry_lines(&entry, 60, Detail::Preview));
        assert!(!rendered.contains('\t'), "{rendered:?}");
        assert!(rendered.contains("     3  }"), "{rendered:?}");
    }

    #[test]
    fn no_tab_reaches_the_buffer_from_a_fenced_code_block() {
        // A model quoting a Makefile or Go, which are indented with tabs.
        let entry = Entry::Assistant {
            id: magi_proto::MessageId::new("a1"),
            text: "```\nbuild:\n\tcargo build\n```".into(),
            thinking: String::new(),
            stop_reason: None,
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        };
        let rendered = cells(&entry_lines(&entry, 60, Detail::Preview));
        assert!(!rendered.contains('\t'), "{rendered:?}");
        assert!(rendered.contains("    cargo build"), "{rendered:?}");
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    #[test]
    fn a_notice_is_marked_so_it_reads_as_magi_and_not_the_model() {
        // It used to be pushed as an assistant message, so `/help` read as the model printing it.
        let lines = entry_lines(
            &Entry::Notice {
                text: "unknown command: /nope".to_owned(),
            },
            40,
            Detail::Preview,
        );
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            rendered.iter().any(|l: &String| l.contains('│')),
            "{rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|l: &String| l.contains("unknown command")),
            "{rendered:?}"
        );
    }

    #[test]
    fn a_notice_renders_its_markdown() {
        // `/help` is markdown, and a notice is where it lands.
        let lines = entry_lines(
            &Entry::Notice {
                text: "**Keys**\n\n- `enter` submit".to_owned(),
            },
            40,
            Detail::Preview,
        );
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(rendered.iter().any(|l: &String| l.contains("Keys")));
        assert!(
            !rendered.iter().any(|l: &String| l.contains("**")),
            "the markers are rendered away: {rendered:?}"
        );
        assert!(rendered.iter().any(|l: &String| l.contains('•')));
    }
}

/// A turn that stopped early, and a rewind.
#[cfg(test)]
#[path = "stopping.rs"]
mod stopping;

#[cfg(test)]
#[path = "spacing.rs"]
mod spacing_tests;
