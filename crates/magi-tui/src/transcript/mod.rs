//! Transcript entries to styled lines. A message, an answer and a tool call are each a [`Block`];
//! a notice and a rule across the transcript are not.

use crate::colour;
use crate::glyph;
use crate::markdown;
use magi_proto::{Entry, StopReason, ToolCallId};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;

mod block;
mod frame;
mod hover;
mod shell;
mod tool;

use frame::{MARGIN, inside};
use tool::STEP;

pub use block::{Block, Head, is_header};
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

/// Whether a row is a gap: nothing but space, and nothing painted behind it.
fn blank_row(line: Option<&Line<'static>>) -> bool {
    line.is_none_or(|line| {
        line.spans.iter().all(|span| {
            span.content.chars().all(char::is_whitespace) && block::ground(span.style).is_none()
        })
    })
}

/// The block `entry` is drawn as, when it is drawn as one.
#[must_use]
pub fn block_of(entry: &Entry, width: u16, detail: Detail) -> Option<Block> {
    match entry {
        Entry::User { text, .. } => Some(said("USER", colour::said_by_you(), text, width)),
        Entry::Assistant {
            text,
            thinking,
            stop_reason,
            error,
            ..
        } => assistant(text, thinking, *stop_reason, error.as_deref(), width),
        Entry::Tool {
            name, args, result, ..
        } => Some(tool::of(name, args, result.as_ref(), width, detail)),
        Entry::From {
            who,
            kin,
            sort,
            text,
        } => Some(from(who, kin, sort, text, width)),
        _ => None,
    }
}

#[must_use]
pub fn entry_lines(entry: &Entry, width: u16, detail: Detail) -> Vec<Line<'static>> {
    if let Some(block) = block_of(entry, width, detail) {
        return block.lines(width);
    }
    match entry {
        Entry::Notice { text } => notice(text, width),
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
        Entry::User { .. } | Entry::Assistant { .. } | Entry::Tool { .. } | Entry::From { .. } => {
            Vec::new()
        }
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

/// The same block as a message of yours, labelled with the name its sender goes by: its role and
/// its id, as the footer shows an agent. One from another project keeps its whole name.
fn from(who: &str, kin: &str, sort: &str, text: &str, width: u16) -> Block {
    // The id alone: the project is this session's own, so printing it repeats a word every message.
    let id = who.rsplit('/').next().unwrap_or(who);
    let label = match kin {
        "elsewhere" => who.to_owned(),
        "myself" | "" => id.to_owned(),
        role => format!("{role}/{id}"),
    };
    // The sort only when it is not the ordinary one, and into the name rather than behind it.
    let label = if sort != "note" && !sort.is_empty() {
        format!("{label} · {sort}")
    } else {
        label
    };
    said(&label, colour::said_by_agent(), text, width)
}

/// A message: its sender's label in the header, and what it says inside.
fn said(label: &str, tag: ratatui::style::Color, text: &str, width: u16) -> Block {
    let head = Head {
        label: label.to_owned(),
        ink: Some(tag),
        copy: true,
        ..Head::default()
    };
    let mut block = Block::new(head, colour::message_bg());
    let style = block.style().fg(colour::message_text());
    block.rows = prose(text, style, width);
    block
}

/// An answer: its thinking, then what it says, then why it stopped if it stopped early. `None`
/// when there is none of the three.
fn assistant(
    text: &str,
    thinking: &str,
    stop_reason: Option<StopReason>,
    error: Option<&str>,
    width: u16,
) -> Option<Block> {
    let head = Head {
        copy: true,
        ..Head::default()
    };
    let mut block = Block::new(head, colour::answer_bg());
    let ground = block.style();
    let mut rows = Vec::new();

    if !thinking.trim().is_empty() {
        let style = ground.fg(colour::thinking()).add_modifier(Modifier::ITALIC);
        rows.extend(prose(thinking.trim(), style, width));
    }
    if !text.trim().is_empty() {
        if !rows.is_empty() {
            rows.push(frame::rule(width, ground));
        }
        rows.extend(prose(text.trim(), ground.fg(colour::text()), width));
    }

    let stopped = match stop_reason {
        Some(StopReason::Length) => Some((
            "Response hit the length limit and stopped here.".to_owned(),
            colour::warning(),
        )),
        Some(StopReason::Aborted) => Some((
            error.map_or_else(|| "Interrupted.".to_owned(), ToOwned::to_owned),
            colour::dim(),
        )),
        Some(StopReason::Error) => Some((
            format!("Error: {}", error.unwrap_or("Unknown error")),
            colour::error(),
        )),
        _ => None,
    };
    if let Some((note, ink)) = stopped {
        if !rows.is_empty() {
            rows.push(frame::breath(width, ground));
        }
        let line = Line::from(Span::styled(note, ground.fg(ink)));
        rows.push(inside(line, width, ground, MARGIN + STEP));
    }

    if rows.is_empty() {
        return None;
    }
    block.rows = rows;
    Some(block)
}

/// Markdown set inside a block, a step in from its fill on both sides.
fn prose(text: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let inner = frame::held(width).saturating_sub(u16::try_from(STEP * 2).unwrap_or(4));
    markdown::render(text, inner, style)
        .into_iter()
        .map(|line| inside(line, width, style, MARGIN + STEP))
        .collect()
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

    /// Whether a row, as text, is a block's header.
    pub(super) fn is_head(row: &str) -> bool {
        row.trim_start().starts_with(crate::glyph::block_edge()) && row.contains("[ ")
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
    fn a_user_message_is_a_full_width_block() {
        let entry = Entry::User {
            images: Vec::new(),
            id: MessageId::new("m1"),
            text: "hello".into(),
            aside: String::new(),
        };
        let lines = entry_lines(&entry, 20, Detail::Preview);
        let rendered = text_of(&lines);
        assert_eq!(rendered.len(), 3, "the header, the body, a row of fill");
        assert_eq!(rendered[1], "    hello           ");
        assert!(is_head(&rendered[0]), "{rendered:?}");
        assert!(rendered[2].trim().is_empty(), "{rendered:?}");
        assert!(!rendered[1].contains('│'), "{rendered:?}");
        assert!(rendered.iter().all(|l| l.chars().count() == 20));
    }

    #[test]
    fn a_user_message_is_labelled_in_its_own_colour() {
        let entry = Entry::User {
            images: Vec::new(),
            id: MessageId::new("m1"),
            text: "hello".into(),
            aside: String::new(),
        };
        let lines = entry_lines(&entry, 20, Detail::Preview);
        let label = lines[0]
            .spans
            .iter()
            .find(|span| span.content == "USER")
            .expect("the label");
        assert_eq!(label.style.fg, Some(colour::said_by_you()));
        assert_eq!(label.style.bg, Some(colour::message_bg()));
    }

    #[test]
    fn a_user_message_is_labelled_in_its_header() {
        let entry = Entry::User {
            images: Vec::new(),
            id: MessageId::new("m1"),
            text: "hello".into(),
            aside: String::new(),
        };
        let rendered = text_of(&entry_lines(&entry, 20, Detail::Preview));
        assert!(rendered[0].contains("[ USER ]"), "{rendered:?}");
        assert_eq!(rendered.len(), 3, "the label grew a row: {rendered:?}");
    }

    #[test]
    fn a_message_from_another_magi_is_tagged_with_who_and_what_they_are() {
        // The two things worth knowing, in one chip.
        let rendered = text_of(&entry_lines(
            &from("magi/alpha-rho", "parent", "note"),
            40,
            Detail::Preview,
        ));
        assert!(rendered[0].contains("[ parent/alpha-rho ]"), "{rendered:?}");
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
    fn a_message_is_labelled_with_the_name_of_who_sent_it() {
        // `MAIN::delta-kappa` told nobody what the sender was: `main` was a fixed word, not a
        // relation. The name an agent goes by is its role and its id, as the footer shows it.
        let rendered = text_of(&entry_lines(
            &from("molla/main/delta-kappa", "explorer", "note"),
            60,
            Detail::Preview,
        ));
        assert!(rendered[0].contains("explorer/delta-kappa"), "{rendered:?}");
        assert!(!rendered[0].contains("::"), "{rendered:?}");
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
    fn an_answer_is_a_block_on_a_fill_of_its_own() {
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
        assert!(is_header(&lines[0]), "{lines:#?}");
        let body = lines
            .iter()
            .find(|line| text_of(std::slice::from_ref(*line))[0].contains("sure"))
            .expect("the answer");
        let inner = &body.spans[1..body.spans.len() - 1];
        assert!(
            inner
                .iter()
                .all(|span| span.style.bg == Some(colour::answer_bg())),
            "{body:#?}"
        );
        assert!(
            text_of(&lines)
                .iter()
                .any(|row| row.starts_with("    sure")),
            "{lines:#?}"
        );
    }

    #[test]
    fn thinking_and_the_answer_share_one_block() {
        let entry = Entry::Assistant {
            id: MessageId::new("m2"),
            text: "sure".into(),
            thinking: "let me see".into(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        };
        let rows = text_of(&entry_lines(&entry, 30, Detail::Preview));
        assert_eq!(
            rows.iter().filter(|row| is_head(row)).count(),
            1,
            "{rows:#?}"
        );
        let thought = rows
            .iter()
            .position(|r| r.contains("let me see"))
            .expect("thinking");
        let answer = rows
            .iter()
            .position(|r| r.contains("sure"))
            .expect("answer");
        assert!(thought < answer, "{rows:#?}");
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
        assert!(
            rendered.iter().any(|row| row.contains("a.rs")),
            "{rendered:?}"
        );
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
                images: Vec::new(),
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
                images: Vec::new(),
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
