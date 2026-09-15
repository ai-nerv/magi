//! A masked tool result, as the provider is shown it.

use super::of_entries;
use magi_model::{Content, Role};
use magi_proto::{Entry, MessageId, ToolCallId, ToolResult};

/// The assistant message a call hangs off.
///
/// Needed in every fixture here: `repair` drops a tool result whose call is not in the message
/// before it, because a provider refuses an orphan — which is the same rule `compact::legal`
/// enforces for a summary cut, arrived at from the other side.
fn spoke(id: &str) -> Entry {
    Entry::Assistant {
        id: MessageId::new(id),
        text: String::new(),
        thinking: String::new(),
        stop_reason: None,
        error: None,
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    }
}

/// A tool call and its answer, as a round journals them.
fn called(id: &str, output: &str) -> Entry {
    Entry::Tool {
        id: ToolCallId::new(id),
        name: "shell".to_owned(),
        args: r#"{"command":"cargo test"}"#.to_owned(),
        result: Some(ToolResult {
            output: output.to_owned(),
            is_error: false,
            shown: None,
        }),
        thought_signature: None,
    }
}

/// What the provider is actually sent, as tool results.
fn results(entries: &[Entry]) -> Vec<String> {
    of_entries(entries)
        .messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .flat_map(|message| message.content.clone())
        .filter_map(|content| match content {
            Content::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .collect()
}

/// Without a mask, the whole output goes.
#[test]
fn an_unmasked_result_is_sent_as_itself() {
    let entries = vec![spoke("a1"), called("c1", "4000 lines of output")];
    assert_eq!(results(&entries), ["4000 lines of output"]);
}

/// **The rung that saves the window.** balthasar tries masking before summarising because it is
/// free and reversible, and tool output is most of a coding session's window.
#[test]
fn a_masked_result_is_sent_as_its_stub() {
    let entries = vec![
        spoke("a1"),
        called("c1", "4000 lines of output"),
        Entry::Masked {
            id: MessageId::new("m1"),
            at: 1,
            shown: "`shell` output elided (~1200 tokens)".to_owned(),
        },
    ];
    assert_eq!(results(&entries), ["`shell` output elided (~1200 tokens)"]);
}

/// The record names one entry, and only that one.
#[test]
fn a_mask_reaches_only_the_entry_it_names() {
    let entries = vec![
        spoke("a1"),
        called("c1", "first output"),
        called("c2", "second output"),
        Entry::Masked {
            id: MessageId::new("m1"),
            at: 2,
            shown: "elided".to_owned(),
        },
    ];
    assert_eq!(results(&entries), ["first output", "elided"]);
}

/// **The call is never masked, only its answer.** A result without the call that made it is an
/// orphan and providers refuse those; masking the call would be the same fault by another route.
#[test]
fn the_call_that_made_it_still_goes() {
    let entries = vec![
        spoke("a1"),
        called("c1", "4000 lines"),
        Entry::Masked {
            id: MessageId::new("m1"),
            at: 1,
            shown: "elided".to_owned(),
        },
    ];
    let calls: Vec<String> = of_entries(&entries)
        .messages
        .iter()
        .flat_map(|message| message.content.clone())
        .filter_map(|content| match content {
            Content::ToolCall { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["shell"], "the call went missing with its output");
}

/// The record itself is bookkeeping and is not a turn of its own.
#[test]
fn the_record_is_not_sent_as_a_message() {
    let entries = vec![
        spoke("a1"),
        called("c1", "output"),
        Entry::Masked {
            id: MessageId::new("m1"),
            at: 1,
            shown: "elided".to_owned(),
        },
    ];
    let said: Vec<String> = of_entries(&entries)
        .messages
        .iter()
        .flat_map(|message| message.content.clone())
        .filter_map(|content| match content {
            Content::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert!(!said.iter().any(|text| text.contains("elided")), "{said:?}");
}

/// **Masking composes with the other two views**, because all three count in one space. A mask on
/// an entry a branch has already dropped is a mask on nothing, and must not shift what is sent.
#[test]
fn a_mask_on_a_branched_away_entry_changes_nothing() {
    let entries = vec![
        spoke("a1"),
        called("c1", "kept output"),
        called("c2", "abandoned output"),
        Entry::Branch {
            id: MessageId::new("b1"),
            keeps: 2,
        },
        Entry::Masked {
            id: MessageId::new("m1"),
            at: 2,
            shown: "elided".to_owned(),
        },
    ];
    assert_eq!(results(&entries), ["kept output"]);
}
