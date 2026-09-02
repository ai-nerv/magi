//! The edges of a block: where it starts, where it stops, and what is set into them.
//!
//! ```text
//! ┌──[ TOOL ]───────────────────────────────[ v ]──┐
//!    …the block's own rows, one column further in…
//! └───────────────────────────────────────────────┘
//! ```
//!
//! **No sides.** A full box costs two columns of every row to draw a line nobody reads,
//! and on a narrow terminal those two columns come out of the text. The top and bottom
//! edges are what say where a block starts and stops; the left and right ones only say it
//! again, forty times a screen.

use super::clip;
use crate::colour;
use crate::glyph;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Columns between a block's frame and everything inside it, on each side.
///
/// Two, not one. The fill used to start against the corner, so the box read as being *drawn on*
/// the text rather than around it — and the edge had nowhere to breathe.
///
/// It is also the left margin for everything that is **not** in a box: an assistant's prose, its
/// thinking, a notice. They are laid out to exactly the span the fill covers, so a screen of
/// mixed blocks and prose has one text column down the left and one down the right, and the only
/// things reaching past them are the frames themselves.
pub(super) const MARGIN: usize = 2;

/// How wide the inside of a block is — and how wide everything outside one is set.
pub(super) fn held(width: u16) -> u16 {
    width.saturating_sub(u16::try_from(MARGIN * 2).unwrap_or(4))
}

/// The top edge of a block, with its name set into it and a handle on the right.
///
/// ```text
/// ┌──[ TOOL ]───────────────────────────────[ v ]──┐
///    …the block's own rows, one column further in…
/// └───────────────────────────────────────────────┘
/// ```
///
/// **No sides.** A full box costs two columns of every row to draw a line nobody reads, and on a
/// narrow terminal those two columns come out of the text. The top and bottom edges are what say
/// where a block starts and stops; the left and right ones only say it again, forty times a
/// screen.
///
/// `handle` is the fold state — `>` shut, `v` open — and is left off entirely for a block that
/// does not fold. A handle on something that cannot be opened is an affordance that lies.
pub(super) fn top(label: &str, chip: Style, handle: Option<&str>, width: u16) -> Line<'static> {
    // **The frame's own colour, which is the prompt box's.** Every drawn line on the screen is
    // one thing — the box you type in and the boxes above it — so they are one colour, and a
    // reader's eye is not asked to sort three greys into a hierarchy that means nothing.
    let edge = Style::default().fg(colour::border());
    // The brackets belong to the frame, not to the name. Only the name carries a colour of its
    // own: what the block *is* is the one thing worth telling apart at a glance, and punctuation
    // painted with it made the whole chip read as the signal.
    let named = format!("[ {label} ]");
    let mut spans = vec![
        Span::styled(glyph::block_top_left().to_owned(), edge),
        Span::styled(glyph::block_edge().repeat(2), edge),
        Span::styled("[ ".to_owned(), edge),
        Span::styled(label.to_owned(), chip),
        Span::styled(" ]".to_owned(), edge),
    ];
    let used = 3 + named.chars().count();

    // The chip, plus two edge cells after it so the handle sits *in* the edge rather than
    // wedged against the corner.
    let worn = handle.map_or(0, |handle| handle.chars().count() + 6);
    // **The name, and then edge.** What a call was *given* used to sit here too, and it made the
    // one row that says what this block is into the row that also says what it was asked — a
    // long path pushed against the handle, and a clipped one said neither thing properly. The
    // arguments are the block's first row now, where they have the width to be read.
    let fill = usize::from(width).saturating_sub(used + worn + 1);
    spans.push(Span::styled(glyph::block_edge().repeat(fill), edge));
    if let Some(handle) = handle {
        // The arrow is the frame's too. It is not *about* this block the way its name is — it is
        // the same affordance on every block that has one, so it belongs to the drawn line rather
        // than standing out from it.
        spans.push(Span::styled(format!("[ {handle} ]"), edge));
        spans.push(Span::styled(glyph::block_edge().repeat(2), edge));
    }
    spans.push(Span::styled(glyph::block_top_right().to_owned(), edge));
    Line::from(spans)
}

/// The bottom edge, corner to corner.
pub(super) fn bottom(width: u16) -> Line<'static> {
    // The block's background is *not* on the edge. The frame is the outer thing and the
    // coloured box sits inside it, so a border painted with the block's own fill would put
    // colour outside the box it is drawing.
    let edge = Style::default().fg(colour::border());
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
    use axon_proto::Entry;
    use axon_proto::{ToolCallId, ToolResult};

    fn tool(detail: Detail, width: u16) -> Vec<String> {
        text_of(&entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
                result: Some(ToolResult {
                    output: "one\ntwo".into(),
                    is_error: false,
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
        // A full box costs two columns of every row to draw a line nobody reads, and on a narrow
        // terminal those two come out of the text.
        for line in tool(Detail::Full, 60).iter().skip(1) {
            assert!(!line.contains('│'), "{line:?}");
        }
    }

    #[test]
    fn every_row_is_exactly_the_width() {
        // The edges and the body are laid out by different code, and a block whose frame is a
        // column wider than its rows is a ragged right margin down the whole transcript.
        for width in [20u16, 33, 60, 120] {
            for line in tool(Detail::Full, width) {
                assert_eq!(
                    line.chars().count(),
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
                .any(|l| l.contains("[ > ]")),
            "a folded block does not offer to open"
        );
        assert!(
            tool(Detail::Full, 60).iter().any(|l| l.contains("[ v ]")),
            "an open block does not offer to fold"
        );
    }

    #[test]
    fn a_block_that_cannot_be_folded_carries_no_handle() {
        // An affordance on something that will not move is an affordance that lies.
        let shown = text_of(&entry_lines(
            &Entry::User {
                id: axon_proto::MessageId::new("m1"),
                text: "hello".into(),
                aside: String::new(),
            },
            60,
            Detail::Preview,
        ));
        assert!(
            !shown[0].contains("[ > ]") && !shown[0].contains("[ v ]"),
            "{shown:#?}"
        );
    }
}

/// One of a block's own rows: the coloured box, shrunk to sit inside the frame.
///
/// The frame is the outer thing and the fill is the inner one. Painted to the full width the
/// background ran out past the corners the edges had just drawn, so the block was a coloured band
/// with a line across the top of it rather than a box with something in it — and on a dark
/// terminal the two ends of every row bled into the margin.
///
/// So the fill spans `1..width-1`, and the two columns the corners stand in are left as the
/// terminal's own. There are no sides drawn in them: the gap is what puts the fill inside.
pub(super) fn inside(line: Line<'static>, width: u16, style: Style, lead: usize) -> Line<'static> {
    let room = usize::from(held(width));
    let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    // `lead` counts from the block's own left edge, and the first `MARGIN` of those columns are
    // outside the fill — so what is left is the padding *within* it.
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

/// The frame is outside, the fill is inside.
#[cfg(test)]
mod nesting {
    use crate::transcript::{Detail, entry_lines};
    use axon_proto::{Entry, MessageId};

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
                    std::iter::repeat_n(span.style.bg.is_some(), span.content.chars().count())
                })
                .collect()
        })
        .collect()
    }

    #[test]
    fn the_fill_stops_short_of_the_frame() {
        // The bug this is here for. Painted to the full width, the background ran out past the
        // corners the edges had just drawn: the block read as a coloured band with a line across
        // the top of it rather than as a box with something in it. It stops two columns short
        // now, on both sides, and that margin is what everything outside a box is set to as well.
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
        // Outside the box means outside: an edge painted with the block's own background is a
        // border drawn *on* the thing it is supposed to contain.
        let rows = filled(30);
        for (at, on) in rows[0].iter().enumerate() {
            // Except the label chip, which carries its own colour because it is a chip.
            assert!(!on || (3..12).contains(&at), "column {at} of the top edge");
        }
        assert!(
            rows[2].iter().all(|on| !on),
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

/// A call with nothing to show yet: one line, no box.
///
/// **A box only when there is something to put in it.** A call stopped on a permission prompt has
/// produced nothing, and framing it drew two edges with a gap between them — an empty box sitting
/// on the screen behind the very question that was holding it up.
///
/// No handle, because nothing is folded away and offering to open it would be offering something
/// that is not there. It grows its box when it has a result.
pub(super) fn lone(label: &str, chip: Style, beside: &str, width: u16) -> Line<'static> {
    let named = format!("[ {label} ]");
    let mut spans = vec![
        Span::raw(" ".repeat(MARGIN)),
        Span::styled("[ ".to_owned(), Style::default().fg(colour::border())),
        Span::styled(label.to_owned(), chip),
        Span::styled(" ]".to_owned(), Style::default().fg(colour::border())),
    ];
    let mut used = MARGIN + named.chars().count();
    if !beside.trim().is_empty() {
        let beside = clip(
            &format!(" {}", beside.trim()),
            usize::from(width).saturating_sub(used),
        );
        used += beside.chars().count();
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
    use axon_proto::{Entry, ToolCallId, ToolResult};

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
        // The one this is here for. A call stopped on a prompt has produced nothing, and framing
        // it drew two edges with a gap between them — an empty box on the screen behind the very
        // question holding it up.
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
                .all(|l| !l.contains("[ > ]") && !l.contains("[ v ]")),
            "{shown:#?}"
        );
    }

    #[test]
    fn a_call_that_produced_nothing_is_not_a_box_either() {
        // Same rule, reached a different way: a `write` that reports nothing has an outcome but
        // no body, and an empty frame says less than a line does.
        let shown = call(Some(ToolResult {
            output: String::new(),
            is_error: false,
        }));
        assert!(shown.iter().all(|l| !l.contains('┌')), "{shown:#?}");
    }

    #[test]
    fn a_call_with_output_grows_its_box() {
        let shown = call(Some(ToolResult {
            output: "one line".into(),
            is_error: false,
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
    use axon_proto::{Entry, MessageId, StopReason, ToolCallId, ToolResult};

    fn said(text: &str) -> Vec<String> {
        text_of(&entry_lines(
            &Entry::Assistant {
                id: MessageId::new("a1"),
                text: text.into(),
                thinking: String::new(),
                stop_reason: Some(StopReason::EndTurn),
                error: None,
                signatures: axon_proto::Signatures::default(),
                usage: axon_proto::Usage::default(),
            },
            40,
            Detail::Preview,
        ))
    }

    #[test]
    fn prose_starts_where_a_block_starts() {
        // They were laid out to different rules, so a screen of mixed output had two ragged
        // margins. An answer and the box above it should begin in the same column.
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
        assert_eq!(
            column(&block[1]),
            column(&prose[1]),
            "{block:#?} against {prose:#?}"
        );
    }

    #[test]
    fn prose_stops_where_a_block_stops() {
        // The right margin too, or long prose runs out past the corner above it.
        let long = "word ".repeat(40);
        for line in said(&long).iter().filter(|l| !l.trim().is_empty()) {
            assert!(
                line.chars().count() <= 40 - super::MARGIN,
                "{line:?} reaches past the frame"
            );
        }
    }

    #[test]
    fn only_the_frame_reaches_the_first_and_last_column() {
        // The whole point of the margin: everything with content in it is inside, and the two
        // outermost columns belong to the edges alone.
        let shown = text_of(&entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
                result: Some(ToolResult {
                    output: "one".into(),
                    is_error: false,
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
