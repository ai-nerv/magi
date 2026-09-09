//! Where a cut may legally fall.

use super::*;
use magi_proto::{MessageId, ToolCallId, ToolResult};

/// `count` plain user turns.
fn plain(count: usize) -> Vec<Entry> {
    (0..count)
        .map(|i| Entry::User {
            id: MessageId::new(format!("u{i}")),
            text: format!("turn {i}"),
            aside: String::new(),
        })
        .collect()
}

/// A tool entry, which is an answer to a call made before it.
fn tool(id: &str) -> Entry {
    Entry::Tool {
        id: ToolCallId::new(id),
        name: "read".to_owned(),
        args: "{}".to_owned(),
        result: Some(ToolResult {
            output: "…".to_owned(),
            is_error: false,
            shown: None,
        }),
        thought_signature: None,
    }
}

/// A cut that lands somewhere harmless is taken as asked.
#[test]
fn a_cut_between_two_turns_is_left_where_it_was_asked_for() {
    assert_eq!(legal(&plain(10), 4), Some(4));
}

/// A cut between an assistant message and the tool result answering it sends the result on its own,
/// which Anthropic answers 400 and the retry classifier calls `Invalid` — nothing recovers.
#[test]
fn a_cut_landing_on_an_answer_moves_past_it() {
    let mut entries = plain(10);
    entries[4] = tool("t1");
    assert_eq!(
        legal(&entries, 4),
        Some(5),
        "the answer must go with the call that made it"
    );
}

/// A whole run of them, not just the first.
#[test]
fn a_cut_landing_in_a_run_of_answers_clears_the_whole_run() {
    let mut entries = plain(12);
    for (at, entry) in entries.iter_mut().enumerate().take(7).skip(3) {
        *entry = tool(&format!("t{at}"));
    }
    assert_eq!(legal(&entries, 3), Some(7));
}

/// Forward, never back: back can reach the start and compact nothing at all, and a cut that
/// removes less than was asked for leaves the window as full as it was.
#[test]
fn the_cut_never_moves_backwards() {
    let mut entries = plain(10);
    entries[5] = tool("t1");
    let cut = legal(&entries, 5).expect("a legal cut");
    assert!(cut >= 5, "moved back to {cut}");
}

/// Everything from the cut to the end being answers means there is nowhere legal to put one.
/// Nothing is compacted rather than something broken being sent.
#[test]
fn a_transcript_ending_in_answers_has_nowhere_legal_to_cut() {
    let mut entries = plain(6);
    for (at, entry) in entries.iter_mut().enumerate().skip(2) {
        *entry = tool(&format!("t{at}"));
    }
    assert_eq!(legal(&entries, 2), None);
}

/// A cut at the very end would summarise the whole conversation and keep nothing.
#[test]
fn a_cut_covering_everything_is_refused() {
    assert_eq!(legal(&plain(4), 4), None);
    assert_eq!(legal(&plain(4), 9), None, "and one past the end");
}

/// A cut at zero is not a compaction.
#[test]
fn a_cut_covering_nothing_is_refused() {
    assert_eq!(legal(&plain(4), 0), None);
}

/// Everything the cut declares replaced is everything the summariser was shown. Computed apart they
/// disagree wherever an entry makes no message, and the difference is tool results dropped silently.
#[test]
fn what_is_declared_replaced_is_what_was_summarised() {
    let mut entries: Vec<Entry> = Vec::new();
    for i in 0..14 {
        entries.push(Entry::User {
            id: MessageId::new(format!("u{i}")),
            text: format!("turn {i}"),
            aside: String::new(),
        });
        if i % 2 == 0 {
            entries.push(Entry::Notice {
                text: "a permission was asked".into(),
            });
        }
    }

    let covered = legal(&entries, entries.len() - 8).expect("there is enough to compact");
    let shown = crate::context::of_entries(&entries[..covered]);
    let kept = crate::context::of_entries(&entries[covered..]);
    let whole = crate::context::of_entries(&entries);

    assert_eq!(
        shown.messages.len() + kept.messages.len(),
        whole.messages.len(),
        "a message was neither summarised nor kept"
    );
    assert!(
        !shown.messages.is_empty(),
        "the summariser was given nothing to summarise"
    );
}
