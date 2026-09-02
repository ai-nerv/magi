//! The edges of a block: where it starts, where it stops, and what is set into them.
//!
//! ```text
//! ┌──[ TOOL ] src/main.rs ─────────────────[ v ]──┐
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

/// How far a block's rows sit in from its frame.
///
/// One column for the edge itself, plus the configured padding — so text starts *inside* the box
/// rather than under the corner it shares a row with.
pub(super) const LEAD: usize = 1 + 1;

/// The top edge of a block, with its name set into it and a handle on the right.
///
/// ```text
/// ┌──[ TOOL ] src/main.rs ─────────────────[ v ]──┐
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
pub(super) fn top(
    label: &str,
    chip: Style,
    beside: Option<&str>,
    handle: Option<&str>,
    width: u16,
    style: Style,
) -> Line<'static> {
    let edge = style.fg(colour::muted());
    let named = format!("[ {label} ]");
    // Two dashes before the name, so it sits off the corner rather than against it.
    let mut spans = vec![
        Span::styled(glyph::block_top_left().to_owned(), edge),
        Span::styled(glyph::block_edge().repeat(2), edge),
        Span::styled(named.clone(), chip),
    ];
    let mut used = 3 + named.chars().count();

    // The chip, plus two edge cells after it so the handle sits *in* the edge rather than
    // wedged against the corner.
    let worn = handle.map_or(0, |handle| handle.chars().count() + 6);
    // One for the closing corner. Everything between the name and the handle is edge, and the
    // summary is clipped into it rather than pushing either end off the screen.
    let room = usize::from(width).saturating_sub(used + worn + 1);
    // Only when there is room for something worth reading. `clip` to nothing still returns the
    // ellipsis it would have ended with, so a screen too narrow for the summary grew a column
    // rather than losing one — and the edge came out a character wider than the block.
    if room > 1
        && let Some(beside) = beside.filter(|beside| !beside.trim().is_empty())
    {
        let beside = clip(&format!(" {} ", beside.trim()), room);
        used += beside.chars().count();
        spans.push(Span::styled(beside, style.fg(colour::dim())));
    }

    let fill = usize::from(width).saturating_sub(used + worn + 1);
    spans.push(Span::styled(glyph::block_edge().repeat(fill), edge));
    if let Some(handle) = handle {
        spans.push(Span::styled(format!("[ {handle} ]"), chip));
        spans.push(Span::styled(glyph::block_edge().repeat(2), edge));
    }
    spans.push(Span::styled(glyph::block_top_right().to_owned(), edge));
    Line::from(spans)
}

/// The bottom edge, corner to corner.
pub(super) fn bottom(width: u16, style: Style) -> Line<'static> {
    let edge = style.fg(colour::muted());
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
        for line in tool(Detail::Full, 60).iter().skip(2) {
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
