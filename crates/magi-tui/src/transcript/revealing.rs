//! Opening a block shows what a preview cut, and nothing ever leaves the frame.
//!
//! Split out under THE RULE; the block renderer next door is what these are about.

use super::*;

const LONG: &str =
    "the quick brown fox jumps over the lazy dog and keeps running well past the edge";

fn rows(detail: Detail, width: u16) -> Vec<String> {
    block(
        "shell",
        r#"{"command":"grep -rn 'a pattern long enough to need cutting' crates/"}"#,
        Some(&magi_proto::ToolResult {
            output: format!("{LONG}\nshort\n{LONG}"),
            is_error: false,
            shown: None,
        }),
        width,
        detail,
    )
    .iter()
    .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
    .collect()
}

#[test]
fn opening_shows_the_end_of_a_line_a_preview_cut() {
    // The complaint this is here for. Every row was cut whichever way the block was showing,
    // so a long line ended in `…` open or shut — and the key that was meant to reveal it
    // added rows underneath without touching the thing being read. On a short result it did
    // nothing visible at all.
    let folded = rows(Detail::Preview, 56).join("\n");
    assert!(folded.contains('…'), "the premise: it was cut\n{folded}");
    assert!(
        !folded.contains("past the edge"),
        "the tail is showing while folded\n{folded}"
    );

    let open = rows(Detail::Full, 56).join("\n");
    assert!(
        open.contains("past the edge"),
        "opening it did not show the rest\n{open}"
    );
    assert!(!open.contains('…'), "still cutting when open\n{open}");
}

#[test]
fn nothing_reaches_past_the_frame_either_way() {
    // The width the body was laid out to subtracted one column on the right where the block
    // keeps two, so a line that filled it came out a character wider than the frame — and
    // the `…` saying it had been cut was the thing hanging past the corner.
    for width in [24u16, 40, 56, 100] {
        for detail in [Detail::Preview, Detail::Full] {
            for row in rows(detail, width) {
                assert_eq!(
                    row.chars().count(),
                    usize::from(width),
                    "at {width} ({detail:?}): {row:?}"
                );
            }
        }
    }
}

#[test]
fn a_preview_is_still_one_row_a_line() {
    // The other half: a preview is a glance, and a wrapped one is not scannable. Three lines
    // of output, three rows, plus the arguments, the seam under them, and the two edges.
    let shown = rows(Detail::Preview, 56);
    assert_eq!(shown.len(), 7, "{shown:#?}");
}

#[test]
fn a_long_argument_is_shown_in_full_when_the_block_is() {
    // The arguments are a row like any other now, so they wrap with the rest.
    let open = rows(Detail::Full, 40).join("\n");
    assert!(
        open.contains("crates/"),
        "the end of the command is missing\n{open}"
    );
}
