//! What a call was given, and how much of it a block shows.
//!
//! Split out under THE RULE; the block renderer next door is what these are about.

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
