//! The fill of a block: the columns it covers, and the rows set inside it. The header that opens a
//! block is [`super::block`]'s.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Columns between a block's edge and everything inside it, on each side. Also the left margin for
/// everything not in a block, so a mixed screen has one text column.
pub(super) const MARGIN: usize = 2;

/// How wide the inside of a block is, and how wide everything outside one is set.
pub(super) fn held(width: u16) -> u16 {
    width.saturating_sub(u16::try_from(MARGIN * 2).unwrap_or(4))
}

/// How far past the margin the fill reaches, on each side.
pub(super) const BLEED: usize = 1;

/// Where a filled row starts, and how wide the colour is.
pub(super) fn fill_of(width: u16) -> (usize, usize) {
    let outer = MARGIN.saturating_sub(BLEED);
    (outer, usize::from(held(width)) + (MARGIN - outer) * 2)
}

/// One of a block's own rows: `line`, `lead` columns in from the block's left edge, on `style`.
/// A span with no colour of its own behind it takes the fill's.
pub(super) fn inside(line: Line<'static>, width: u16, style: Style, lead: usize) -> Line<'static> {
    let (outer, room) = fill_of(width);
    let used: usize = line
        .spans
        .iter()
        .map(|s| crate::wrap::columns(&s.content))
        .sum();
    let pad = lead.saturating_sub(outer).min(room);
    let trailing = room.saturating_sub(used + pad);

    let mut spans = vec![
        Span::raw(" ".repeat(outer)),
        Span::styled(" ".repeat(pad), style),
    ];
    spans.extend(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content, style.patch(span.style))),
    );
    spans.push(Span::styled(" ".repeat(trailing), style));
    spans.push(Span::raw(" ".repeat(outer)));
    Line::from(spans)
}

/// A row of nothing but the block's own fill.
pub(super) fn breath(width: u16, style: Style) -> Line<'static> {
    inside(Line::default(), width, style, MARGIN)
}

/// The seam between two parts of a block, inside the fill.
pub(super) fn rule(width: u16, style: Style) -> Line<'static> {
    let (outer, room) = fill_of(width);
    let span = room.saturating_sub(2);
    Line::from(vec![
        Span::raw(" ".repeat(outer)),
        Span::styled(" ", style),
        Span::styled(
            crate::glyph::seam().repeat(span),
            style.fg(crate::colour::tool_seam()),
        ),
        Span::styled(" ", style),
        Span::raw(" ".repeat(outer)),
    ])
}

/// Where a block starts and stops, and that every row of it is the width.
#[cfg(test)]
mod framing {
    use crate::transcript::Detail;
    use crate::transcript::entry_lines;
    use crate::transcript::tests::{is_head, text_of};
    use magi_proto::Entry;
    use magi_proto::{ToolCallId, ToolResult};

    fn tool(detail: Detail, width: u16) -> Vec<String> {
        text_of(&entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
                result: Some(ToolResult {
                    images: Vec::new(),
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
    fn a_block_is_a_header_a_body_and_a_closing_row_of_fill() {
        let shown = tool(Detail::Full, 60);
        assert!(is_head(&shown[0]), "{shown:#?}");
        let last = shown.last().expect("a closing row");
        assert!(last.trim().is_empty(), "{shown:#?}");
        assert_eq!(
            shown.iter().filter(|row| is_head(row)).count(),
            1,
            "{shown:#?}"
        );
    }

    #[test]
    fn nothing_is_drawn_down_the_sides() {
        for line in tool(Detail::Full, 60).iter().skip(1) {
            assert!(!line.contains('│'), "{line:?}");
        }
    }

    #[test]
    fn every_row_is_exactly_the_width() {
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
    fn the_body_sits_inside_the_fill() {
        let shown = tool(Detail::Full, 60);
        let body = shown.iter().find(|l| l.contains("one")).expect("the body");
        assert!(body.starts_with("    one"), "{body:?}");
    }

    #[test]
    fn the_handle_says_which_way_the_block_will_go() {
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
        let shown = text_of(&entry_lines(
            &Entry::User {
                images: Vec::new(),
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

/// The fill stands inside the screen's outermost columns, header included.
#[cfg(test)]
mod nesting {
    use crate::transcript::block::ground;
    use crate::transcript::{Detail, entry_lines};
    use magi_proto::{Entry, MessageId};

    /// Every column of a rendered row, and whether a colour is painted behind it.
    fn filled(width: u16) -> Vec<Vec<bool>> {
        entry_lines(
            &Entry::User {
                images: Vec::new(),
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
                        ground(span.style).is_some(),
                        crate::wrap::columns(&span.content),
                    )
                })
                .collect()
        })
        .collect()
    }

    #[test]
    fn the_fill_stands_one_column_inside_each_end() {
        for row in filled(30) {
            assert!(!row[0], "the fill reaches the very first column");
            assert!(!row[29], "and the very last");
            assert!(row[1..29].iter().all(|on| *on), "{row:?}");
        }
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

/// A call is a block from the moment it is made.
#[cfg(test)]
mod pending {
    use crate::transcript::tests::{is_head, text_of};
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
    fn a_call_still_out_is_a_block_with_what_it_was_given() {
        let shown = call(None);
        assert!(is_head(&shown[0]), "{shown:#?}");
        assert!(shown[0].contains("[ shell ]"), "{shown:#?}");
        assert!(
            shown[0].contains(&format!("[ {} ]", crate::glyph::running())),
            "{shown:#?}"
        );
        assert!(shown.iter().any(|l| l.contains("git log -1")), "{shown:#?}");
    }

    #[test]
    fn a_call_that_produced_nothing_says_how_it_ended() {
        let shown = call(Some(ToolResult {
            images: Vec::new(),
            output: String::new(),
            is_error: false,
            shown: None,
        }));
        assert!(
            shown[0].contains(&format!("[ {} ]", crate::glyph::outcome_ok())),
            "{shown:#?}"
        );
        assert!(shown.iter().any(|l| l.contains("git log -1")), "{shown:#?}");
    }

    #[test]
    fn a_call_with_output_shows_it_under_what_it_was_given() {
        let shown = call(Some(ToolResult {
            images: Vec::new(),
            output: "one line".into(),
            is_error: false,
            shown: None,
        }));
        let asked = shown
            .iter()
            .position(|l| l.contains("git log -1"))
            .expect("the command");
        let said = shown
            .iter()
            .position(|l| l.contains("one line"))
            .expect("the output");
        assert!(asked < said, "{shown:#?}");
    }
}

/// Prose and blocks share one text column.
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
    fn an_answer_starts_where_a_question_starts() {
        let block = text_of(&entry_lines(
            &Entry::User {
                images: Vec::new(),
                id: MessageId::new("m1"),
                text: "hello".into(),
                aside: String::new(),
            },
            40,
            Detail::Preview,
        ));
        let prose = said("hello");
        let column = |line: &str| line.len() - line.trim_start().len();
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
    fn an_answer_stops_inside_its_block() {
        let long = "word ".repeat(40);
        for line in said(&long).iter().skip(1) {
            assert!(
                line.trim_end().chars().count() <= 40 - super::MARGIN,
                "{line:?} reaches past the fill"
            );
        }
    }

    #[test]
    fn nothing_reaches_the_first_column() {
        let shown = text_of(&entry_lines(
            &Entry::Tool {
                id: ToolCallId::new("t1"),
                name: "read".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
                result: Some(ToolResult {
                    images: Vec::new(),
                    output: "one".into(),
                    is_error: false,
                    shown: None,
                }),
                thought_signature: None,
            },
            40,
            Detail::Full,
        ));
        for line in &shown {
            let first = line.chars().next().expect("a column");
            assert_eq!(first, ' ', "{line:?} starts in the screen's column");
        }
    }
}

/// A glyph two columns wide does not push a row past the fill.
#[cfg(test)]
mod wide {
    use crate::transcript::tests::text_of;
    use crate::transcript::{Detail, entry_lines};
    use magi_proto::{Entry, MessageId, ToolCallId, ToolResult};

    /// Two columns each on a terminal, one `char` each in Rust.
    const WIDE: &str = "日本語のテキストがここにあります、これは長い行です";

    /// Measured with the width table itself, never with the code under test.
    fn width_of(line: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(line)
    }

    #[test]
    fn a_message_of_wide_glyphs_still_fills_the_width_exactly() {
        for width in [24u16, 40, 56] {
            let shown = text_of(&entry_lines(
                &Entry::User {
                    images: Vec::new(),
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
                        images: Vec::new(),
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
