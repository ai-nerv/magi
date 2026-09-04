//! Drawing what a tool meant, rather than guessing at it from the text.
//!
//! Split out under THE RULE; the block renderer next door is what these are about.

use super::*;

use magi_proto::tooling::{Role, Shown, Span as Painted};

/// A block whose result carries `shown`, at `detail`.
fn block_of(shown: Option<Shown>, output: &str, detail: Detail) -> Vec<Line<'static>> {
    block(
        "patch",
        "{}",
        Some(&magi_proto::ToolResult {
            output: output.to_owned(),
            is_error: false,
            shown,
        }),
        60,
        detail,
    )
}

/// The colour of the first span on the row holding `text`.
fn colour_of(lines: &[Line<'static>], text: &str) -> Option<ratatui::style::Color> {
    lines
        .iter()
        .find(|line| line.spans.iter().any(|s| s.content.contains(text)))?
        .spans
        .iter()
        .find(|s| s.content.contains(text))?
        .style
        .fg
}

#[test]
fn a_painted_result_is_drawn_in_the_role_it_named() {
    let painted = Shown::Painted {
        lines: vec![
            vec![Painted::new(Role::Removed, "-was")],
            vec![Painted::new(Role::Added, "+now")],
        ],
    };
    let lines = block_of(Some(painted), "-was\n+now", Detail::Full);
    assert_eq!(colour_of(&lines, "-was"), Some(colour::diff_removed()));
    assert_eq!(colour_of(&lines, "+now"), Some(colour::diff_added()));
}

#[test]
fn a_role_the_guesser_would_have_got_wrong_is_drawn_as_the_tool_meant_it() {
    // The whole point of carrying meaning rather than reading the first character. This is
    // `git log --oneline`: a line starting with `-` that is not a removal, which the
    // fallback colours red and a tool that says otherwise does not.
    let painted = Shown::Painted {
        lines: vec![vec![Painted::new(Role::Text, "- not a diff at all")]],
    };
    let lines = block_of(Some(painted), "- not a diff at all", Detail::Full);
    assert_eq!(colour_of(&lines, "not a diff"), Some(colour::text()));

    // And without the tool saying so, the guess still stands — which is what every tool
    // that has no view relies on.
    let guessed = block_of(None, "- not a diff at all", Detail::Full);
    assert_eq!(
        colour_of(&guessed, "not a diff"),
        Some(colour::diff_removed())
    );
}

#[test]
fn a_painted_row_keeps_a_colour_per_span() {
    // A highlighted `cat` is many roles on one line, and a renderer that took the first
    // would paint the whole line as its first token.
    let painted = Shown::Painted {
        lines: vec![vec![
            Painted::new(Role::Keyword, "fn"),
            Painted::new(Role::Text, " "),
            Painted::new(Role::Func, "main"),
        ]],
    };
    let lines = block_of(Some(painted), "fn main", Detail::Full);
    assert_eq!(colour_of(&lines, "fn"), Some(colour::md_heading()));
    assert_eq!(colour_of(&lines, "main"), Some(colour::accent()));
}

#[test]
fn a_failed_call_is_still_drawn_as_failed() {
    // The outcome outranks the paint: a result that went wrong should read as wrong at a
    // glance, whatever roles it happened to carry.
    let lines = block(
        "patch",
        "{}",
        Some(&magi_proto::ToolResult {
            output: "+now".to_owned(),
            is_error: true,
            shown: Some(Shown::Painted {
                lines: vec![vec![Painted::new(Role::Added, "+now")]],
            }),
        }),
        60,
        Detail::Full,
    );
    assert_eq!(colour_of(&lines, "+now"), Some(colour::tool_failed()));
}

#[test]
fn a_question_is_not_drawn_as_output() {
    // It is drawn by whoever can answer it. A block that rendered it as text would put a
    // picker on screen that nobody could use.
    let asking = Shown::Ask(magi_proto::tooling::Ask {
        question: "run it?".to_owned(),
        options: Vec::new(),
        detail: Vec::new(),
    });
    let lines = block_of(Some(asking), "", Detail::Full);
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect();
    assert!(!text.contains("run it?"), "{text}");
}

#[test]
fn a_preview_still_cuts_a_painted_row_to_the_width() {
    // Nothing may reach past the frame, however many spans it arrived in.
    let long = "x".repeat(200);
    let painted = Shown::Painted {
        lines: vec![vec![Painted::new(Role::Added, long.clone())]],
    };
    for line in block_of(Some(painted), &long, Detail::Preview) {
        let wide: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        assert!(wide <= 60, "{wide} columns on a 60-wide block");
    }
}

#[test]
fn a_painted_row_sits_on_the_block_it_is_in() {
    // The bug: roles were resolved onto `Style::default()`, which has no background, so every
    // syntax-highlighted row came out with the terminal showing through it while the plain rows
    // either side kept the block's fill. A `cat` read as a block with holes punched in it.
    let painted = Shown::Painted {
        lines: vec![vec![Painted::new(Role::Keyword, "fn")]],
    };
    let fill = |lines: &[Line<'static>]| {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("fn"))
            .and_then(|span| span.style.bg)
    };
    assert_eq!(
        fill(&block_of(Some(painted), "fn", Detail::Full)),
        fill(&block_of(None, "fn", Detail::Full)),
        "a painted row lost the block's fill"
    );
}
