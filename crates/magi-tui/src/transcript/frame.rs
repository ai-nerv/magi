//! The edges of a block: where it starts, where it stops, and what is set into them.
//!
//! ```text
//! ┌──[ TOOL ]───────────────────────────────[ v ]──┐
//!    …the block's own rows, one column further in…
//! └───────────────────────────────────────────────┘
//! ```
//!
//! No sides: they would cost two columns of every row, taken out of the text on a narrow terminal.

use super::clip;
use crate::colour;
use crate::glyph;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Columns between a block's frame and everything inside it, on each side. Also the left margin for
/// everything not in a box — prose, thinking, notices — so a mixed screen has one text column.
pub(super) const MARGIN: usize = 2;

/// How wide the inside of a block is — and how wide everything outside one is set.
pub(super) fn held(width: u16) -> u16 {
    width.saturating_sub(u16::try_from(MARGIN * 2).unwrap_or(4))
}

/// The top edge of a block, with its name set into it and a handle on the right. `handle` is the
/// fold state — `>` shut, `v` open — left off entirely for a block that does not fold. `copy` puts
/// a second chip inboard of the handle. `mark` is a chip right after the name: `·` while the call
/// is out, `✓` or `✗` when it lands.
pub(super) fn top(
    label: &str,
    chip: Style,
    mark: Option<(&str, Style)>,
    handle: Option<&str>,
    copy: bool,
    width: u16,
) -> Line<'static> {
    // A block's frame is not the prompt's border: the record sits further back than what you type in.
    let edge = Style::default().fg(colour::block_frame());
    // The brackets belong to the frame, not to the name; only the name carries a colour of its own.
    let mut spans = vec![
        Span::styled(glyph::block_top_left().to_owned(), edge),
        Span::styled(glyph::block_edge().repeat(2), edge),
    ];
    // No name, no chip. An empty label drew `[  ]`, a bracket around nothing.
    let mut used = if label.is_empty() {
        3
    } else {
        spans.push(Span::styled("[ ".to_owned(), edge));
        spans.push(Span::styled(label.to_owned(), chip));
        spans.push(Span::styled(" ]".to_owned(), edge));
        3 + crate::wrap::columns(&format!("[ {label} ]"))
    };
    // The chips, in the order they are given up when the edge runs out — the handle last, because
    // folding depends on it. Pushing them anyway made the row wider than the frame.
    let held = handle.map_or(0, |handle| crate::wrap::columns(handle) + 6);
    let worn_mark = mark.map_or(0, |(mark, _)| crate::wrap::columns(mark) + 6);
    let worn_copy = crate::wrap::columns(glyph::copy()) + 6;
    let room = usize::from(width);
    let copy = copy && used + held + worn_mark + worn_copy < room;
    let mark = mark.filter(|_| used + held + worn_mark < room);

    if let Some((glyph, ink)) = mark {
        // Two edge cells between the two chips, so the pair reads as two things, not one long tag.
        spans.push(Span::styled(glyph::block_edge().repeat(2), edge));
        spans.push(Span::styled("[ ".to_owned(), edge));
        spans.push(Span::styled(glyph.to_owned(), ink));
        spans.push(Span::styled(" ]".to_owned(), edge));
        used += 2 + crate::wrap::columns(&format!("[ {glyph} ]"));
    }

    let worn = held + usize::from(copy) * worn_copy;
    // The name, and then edge. What a call was given is the block's first row, not this one.
    let fill = usize::from(width).saturating_sub(used + worn + 1);
    spans.push(Span::styled(glyph::block_edge().repeat(fill), edge));
    if copy {
        // The frame's, like the handle: the same affordance on every block that has one.
        spans.push(Span::styled(format!("[ {} ]", glyph::copy()), edge));
        spans.push(Span::styled(glyph::block_edge().repeat(2), edge));
    }
    if let Some(handle) = handle {
        // The arrow is the frame's too, not about this block the way its name is.
        spans.push(Span::styled(format!("[ {handle} ]"), edge));
        spans.push(Span::styled(glyph::block_edge().repeat(2), edge));
    }
    spans.push(Span::styled(glyph::block_top_right().to_owned(), edge));
    Line::from(spans)
}

/// The bottom edge, corner to corner. Plain: what became of a call is drawn beside the call itself,
/// at the top of the block.
pub(super) fn bottom(width: u16) -> Line<'static> {
    let edge = Style::default().fg(colour::block_frame());
    Line::from(vec![
        Span::styled(glyph::block_bottom_left().to_owned(), edge),
        Span::styled(
            glyph::block_edge().repeat(usize::from(width).saturating_sub(2)),
            edge,
        ),
        Span::styled(glyph::block_bottom_right().to_owned(), edge),
    ])
}

/// The frame: where a block starts, where it stops, and that nothing runs under its edges.
#[cfg(test)]
mod framing {
    use crate::transcript::Detail;
    use crate::transcript::entry_lines;
    use crate::transcript::tests::text_of;
    use magi_proto::Entry;
    use magi_proto::{ToolCallId, ToolResult};

    fn tool(detail: Detail, width: u16) -> Vec<String> {
        text_of(&entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
                result: Some(ToolResult {
                    output: "one\ntwo".into(),
                    is_error: false,
                    shown: None,
                }),
                thought_signature: None,
            },
            width,
            detail,
        ))
    }

    #[test]
    fn a_block_is_a_top_edge_a_body_and_a_bottom_edge() {
        let shown = tool(Detail::Full, 60);
        let top = shown
            .iter()
            .position(|l| l.starts_with('┌'))
            .expect("a top");
        assert!(shown[top].ends_with('┐'), "{shown:#?}");
        let bottom = shown.last().expect("a bottom");
        assert!(
            bottom.starts_with('└') && bottom.ends_with('┘'),
            "{shown:#?}"
        );
    }

    #[test]
    fn nothing_is_drawn_down_the_sides() {
        // A full box costs two columns of every row, out of the text on a narrow terminal.
        for line in tool(Detail::Full, 60).iter().skip(1) {
            assert!(!line.contains('│'), "{line:?}");
        }
    }

    #[test]
    fn every_row_is_exactly_the_width() {
        // Edges and body are laid out by different code, and a mismatch is a ragged right margin.
        for width in [20u16, 33, 60, 120] {
            for line in tool(Detail::Full, width) {
                assert_eq!(
                    crate::wrap::columns(&line),
                    usize::from(width),
                    "at {width}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn the_body_sits_inside_the_edges() {
        // One column in, so text does not run under the corner it shares a row with.
        let shown = tool(Detail::Full, 60);
        let body = shown.iter().find(|l| l.contains("one")).expect("the body");
        assert!(body.starts_with("    one"), "{body:?}");
    }

    #[test]
    fn the_handle_says_which_way_the_block_will_go() {
        // `>` on a shut block, `v` on an open one: what the key will do, not what state it is in.
        assert!(
            tool(Detail::Preview, 60)
                .iter()
                .any(|l| l.contains(&format!("[ {} ]", crate::glyph::expand()))),
            "a folded block does not offer to open"
        );
        assert!(
            tool(Detail::Full, 60)
                .iter()
                .any(|l| l.contains(&format!("[ {} ]", crate::glyph::collapse()))),
            "an open block does not offer to fold"
        );
    }

    #[test]
    fn a_block_that_cannot_be_folded_carries_no_handle() {
        // An affordance on something that will not move is an affordance that lies.
        let shown = text_of(&entry_lines(
            &Entry::User {
                id: magi_proto::MessageId::new("m1"),
                text: "hello".into(),
                aside: String::new(),
            },
            60,
            Detail::Preview,
        ));
        assert!(
            !shown[0].contains(&format!("[ {} ]", crate::glyph::expand()))
                && !shown[0].contains(&format!("[ {} ]", crate::glyph::collapse())),
            "{shown:#?}"
        );
    }
}

/// One of a block's own rows: the coloured box, shrunk to sit inside the frame. The fill spans
/// `1..width-1`, leaving the corners' two columns as the terminal's own; the gap puts it inside.
pub(super) fn inside(line: Line<'static>, width: u16, style: Style, lead: usize) -> Line<'static> {
    let room = usize::from(held(width));
    let used: usize = line
        .spans
        .iter()
        .map(|s| crate::wrap::columns(&s.content))
        .sum();
    // `lead` counts from the block's left edge and the first `MARGIN` are outside the fill.
    let pad = lead.saturating_sub(MARGIN).min(room);
    let trailing = room.saturating_sub(used + pad);

    let mut spans = vec![
        Span::raw(" ".repeat(MARGIN)),
        Span::styled(" ".repeat(pad), style),
    ];
    spans.extend(line.spans);
    spans.push(Span::styled(" ".repeat(trailing), style));
    spans.push(Span::raw(" ".repeat(MARGIN)));
    Line::from(spans)
}

/// A row of nothing but the block's own fill, one under the top edge and one above the bottom.
/// Filled rather than skipped: a bare blank row reads as a gap between two blocks.
pub(super) fn breath(width: u16, style: Style) -> Line<'static> {
    inside(Line::default(), width, style, MARGIN)
}

/// The seam between what a call was asked and what it answered, inside the fill rather than across it.
pub(super) fn rule(width: u16, style: Style) -> Line<'static> {
    let room = usize::from(held(width));
    // One column of fill at each end, or the rule meets the frame and makes two boxes.
    let span = room.saturating_sub(2);
    Line::from(vec![
        Span::raw(" ".repeat(MARGIN)),
        Span::styled(" ", style),
        Span::styled("─".repeat(span), style.fg(crate::colour::tool_seam())),
        Span::styled(" ", style),
        Span::raw(" ".repeat(MARGIN)),
    ])
}

/// The frame is outside, the fill is inside.
#[cfg(test)]
mod nesting {
    use crate::transcript::{Detail, entry_lines};
    use magi_proto::{Entry, MessageId};

    /// Every column of a rendered row, and whether the block's fill is painted behind it.
    fn filled(width: u16) -> Vec<Vec<bool>> {
        entry_lines(
            &Entry::User {
                id: MessageId::new("m1"),
                text: "hello".into(),
                aside: String::new(),
            },
            width,
            Detail::Preview,
        )
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .flat_map(|span| {
                    std::iter::repeat_n(
                        span.style.bg.is_some(),
                        crate::wrap::columns(&span.content),
                    )
                })
                .collect()
        })
        .collect()
    }

    #[test]
    fn the_fill_stops_short_of_the_frame() {
        // Painted to the full width, the background ran out past the corners the edges had drawn.
        let rows = filled(30);
        let body = &rows[1];
        assert!(
            !body[0] && !body[1],
            "the fill runs into the frame's margin"
        );
        assert!(!body[28] && !body[29], "and out the other side");
        assert!(
            body[2] && body[27],
            "the fill should span everything between"
        );
    }

    #[test]
    fn the_edges_carry_no_fill_of_their_own() {
        // An edge painted with the block's own background is a border drawn on what it contains.
        let rows = filled(30);
        for (at, on) in rows[0].iter().enumerate() {
            // Except the label chip, which carries its own colour because it is a chip.
            assert!(!on || (3..12).contains(&at), "column {at} of the top edge");
        }
        assert!(
            rows.last().expect("a bottom edge").iter().all(|on| !on),
            "the bottom edge is painted with the block's fill"
        );
    }

    #[test]
    fn a_row_is_still_exactly_the_width() {
        for width in [12u16, 30, 80] {
            for row in filled(width) {
                assert_eq!(row.len(), usize::from(width), "at {width}");
            }
        }
    }
}

/// A call with nothing to show yet: one line, no box, because framing it drew two edges with a gap
/// between them. No handle, since nothing is folded away. It grows its box when it has a result.
pub(super) fn lone(label: &str, chip: Style, beside: &str, width: u16) -> Line<'static> {
    let named = format!("[ {label} ]");
    // A call with no result yet is the one still out, so this row always wears the running dot.
    let waiting = format!("{} ", glyph::running());
    let mut spans = vec![
        Span::raw(" ".repeat(MARGIN)),
        Span::styled(waiting.clone(), Style::default().fg(colour::tool_title())),
        Span::styled("[ ".to_owned(), Style::default().fg(colour::block_frame())),
        Span::styled(label.to_owned(), chip),
        Span::styled(" ]".to_owned(), Style::default().fg(colour::block_frame())),
    ];
    let mut used = MARGIN + crate::wrap::columns(&waiting) + crate::wrap::columns(&named);
    if !beside.trim().is_empty() {
        let beside = clip(
            &format!(" {}", beside.trim()),
            usize::from(width).saturating_sub(used),
        );
        used += crate::wrap::columns(&beside);
        spans.push(Span::styled(beside, Style::default().fg(colour::dim())));
    }
    spans.push(Span::raw(
        " ".repeat(usize::from(width).saturating_sub(used)),
    ));
    Line::from(spans.clone())
}

/// A box is drawn only when there is something to put in it.
#[cfg(test)]
mod emptiness {
    use crate::transcript::tests::text_of;
    use crate::transcript::{Detail, entry_lines};
    use magi_proto::{Entry, ToolCallId, ToolResult};

    fn call(result: Option<ToolResult>) -> Vec<String> {
        text_of(&entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "shell".into(),
                args: r#"{"command":"git log -1"}"#.into(),
                result,
                thought_signature: None,
            },
            56,
            Detail::Preview,
        ))
    }

    #[test]
    fn a_call_waiting_on_a_permission_is_not_a_box() {
        // A call stopped on a prompt has produced nothing, and framing it drew an empty box.
        let shown = call(None);
        assert!(
            shown.iter().all(|l| !l.contains('┌') && !l.contains('└')),
            "an empty box was drawn: {shown:#?}"
        );
        assert!(
            shown.iter().any(|l| l.contains("[ shell ]")),
            "and it says nothing about what is being asked: {shown:#?}"
        );
    }

    #[test]
    fn nor_does_it_offer_a_handle() {
        // Nothing is folded away. Offering to open it would be offering something not there.
        let shown = call(None);
        assert!(
            shown
                .iter()
                .all(|l| !l.contains(crate::glyph::expand())
                    && !l.contains(crate::glyph::collapse())),
            "{shown:#?}"
        );
    }

    #[test]
    fn a_call_that_produced_nothing_is_not_a_box_either() {
        // A `write` that reports nothing has an outcome but no body.
        let shown = call(Some(ToolResult {
            output: String::new(),
            is_error: false,
            shown: None,
        }));
        assert!(shown.iter().all(|l| !l.contains('┌')), "{shown:#?}");
    }

    #[test]
    fn a_call_with_output_grows_its_box() {
        let shown = call(Some(ToolResult {
            output: "one line".into(),
            is_error: false,
            shown: None,
        }));
        assert!(shown.iter().any(|l| l.contains('┌')), "{shown:#?}");
        assert!(shown.iter().any(|l| l.contains('└')), "{shown:#?}");
        assert!(shown.iter().any(|l| l.contains("one line")), "{shown:#?}");
    }
}

/// Prose and blocks share one text column, so only the frames reach past it.
#[cfg(test)]
mod alignment {
    use crate::transcript::tests::text_of;
    use crate::transcript::{Detail, entry_lines};
    use magi_proto::{Entry, MessageId, StopReason, ToolCallId, ToolResult};

    fn said(text: &str) -> Vec<String> {
        text_of(&entry_lines(
            &Entry::Assistant {
                id: MessageId::new("a1"),
                text: text.into(),
                thinking: String::new(),
                stop_reason: Some(StopReason::EndTurn),
                error: None,
                signatures: magi_proto::Signatures::default(),
                usage: magi_proto::Usage::default(),
            },
            40,
            Detail::Preview,
        ))
    }

    #[test]
    fn prose_starts_where_a_block_starts() {
        // An answer and the box above it should begin in the same column.
        let block = text_of(&entry_lines(
            &Entry::User {
                id: MessageId::new("m1"),
                text: "hello".into(),
                aside: String::new(),
            },
            40,
            Detail::Preview,
        ));
        let prose = said("hello");
        let column = |line: &str| line.len() - line.trim_start().len();
        // Found by the text, not by a row number: a block pads inside its frame and prose does not.
        let saying = |rows: &[String]| {
            rows.iter()
                .find(|row| row.contains("hello"))
                .expect("the row that says it")
                .clone()
        };
        assert_eq!(
            column(&saying(&block)),
            column(&saying(&prose)),
            "{block:#?} against {prose:#?}"
        );
    }

    #[test]
    fn prose_stops_where_a_block_stops() {
        // The right margin too. The edges are exempt: a frame spans the whole width.
        let long = "word ".repeat(40);
        let framing = |line: &str| line.starts_with('┌') || line.starts_with('└');
        for line in said(&long)
            .iter()
            .filter(|l| !l.trim().is_empty() && !framing(l))
        {
            assert!(
                line.chars().count() <= 40 - super::MARGIN,
                "{line:?} reaches past the frame"
            );
        }
    }

    #[test]
    fn only_the_frame_reaches_the_first_and_last_column() {
        // Everything with content is inside, and the two outermost columns belong to the edges.
        let shown = text_of(&entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
                result: Some(ToolResult {
                    output: "one".into(),
                    is_error: false,
                    shown: None,
                }),
                thought_signature: None,
            },
            40,
            Detail::Full,
        ));
        for line in shown.iter().filter(|l| !l.trim().is_empty()) {
            let edge = line.starts_with('┌') || line.starts_with('└');
            if !edge {
                let first = line.chars().next().expect("a column");
                assert_eq!(first, ' ', "{line:?} starts in the frame's column");
            }
        }
    }
}

/// A glyph two columns wide does not push a row past the frame.
#[cfg(test)]
mod wide {
    use crate::transcript::tests::text_of;
    use crate::transcript::{Detail, entry_lines};
    use magi_proto::{Entry, MessageId, ToolCallId, ToolResult};

    /// Two columns each on a terminal, one `char` each in Rust — which is the whole problem.
    const WIDE: &str = "日本語のテキストがここにあります、これは長い行です";

    /// Measured with the width table itself, never with the code under test.
    fn width_of(line: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(line)
    }

    #[test]
    fn a_message_of_wide_glyphs_still_fills_the_width_exactly() {
        // Everything counted characters, so one wide glyph pushed a row a column past the frame.
        for width in [24u16, 40, 56] {
            let shown = text_of(&entry_lines(
                &Entry::User {
                    id: MessageId::new("m1"),
                    text: WIDE.into(),
                    aside: String::new(),
                },
                width,
                Detail::Preview,
            ));
            for line in shown {
                assert_eq!(width_of(&line), usize::from(width), "at {width}: {line:?}");
            }
        }
    }

    #[test]
    fn wide_tool_output_fills_the_width_both_folded_and_open() {
        for detail in [Detail::Preview, Detail::Full] {
            let shown = text_of(&entry_lines(
                &Entry::Tool {
                    id: ToolCallId::new("t1"),
                    name: "shell".into(),
                    args: format!(r#"{{"command":"echo {WIDE}"}}"#),
                    result: Some(ToolResult {
                        output: format!("{WIDE}\n{WIDE}"),
                        is_error: false,
                        shown: None,
                    }),
                    thought_signature: None,
                },
                48,
                detail,
            ));
            for line in shown {
                assert_eq!(width_of(&line), 48, "{detail:?}: {line:?}");
            }
        }
    }

    #[test]
    fn a_cut_ends_no_wider_than_it_was_asked_for() {
        // Cutting at `width - 1` characters produced a run wider than the budget for a wide glyph.
        for room in 4..20 {
            let cut = super::super::clip(WIDE, room);
            assert!(
                width_of(&cut) <= room,
                "{room}: {cut:?} is {}",
                width_of(&cut)
            );
        }
    }

    #[test]
    fn wrapping_wide_text_never_overflows_a_row() {
        let rows = crate::wrap::line(ratatui::text::Line::from(WIDE), 10);
        for row in rows {
            let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(width_of(&text) <= 10, "{text:?} is {}", width_of(&text));
        }
    }
}
