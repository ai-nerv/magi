//! What a stack keeps, what it draws again, and where each block lands.

use super::*;
use magi_proto::tooling::Span;
use magi_proto::{MessageId, ToolResult};

fn user(text: &str) -> Entry {
    Entry::User {
        images: Vec::new(),
        id: MessageId::new(text),
        text: text.into(),
        aside: String::new(),
    }
}

fn answer(text: &str) -> Entry {
    Entry::Assistant {
        id: MessageId::new("a"),
        text: text.into(),
        thinking: String::new(),
        stop_reason: None,
        error: None,
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    }
}

fn call(id: &str) -> Entry {
    Entry::Tool {
        id: ToolCallId::new(id),
        name: "shell".into(),
        args: r#"{"command":"ls"}"#.into(),
        result: Some(ToolResult {
            images: Vec::new(),
            output: "out".into(),
            is_error: false,
            shown: None,
        }),
        thought_signature: None,
    }
}

fn stacked(entries: &[Entry]) -> Stack {
    let mut stack = Stack::new();
    stack.update(entries, 40, Detail::Preview, &BTreeSet::new(), &[]);
    stack
}

#[test]
fn an_unchanged_transcript_draws_nothing_again() {
    let entries = [user("one"), answer("two"), call("t1")];
    let mut stack = stacked(&entries);
    assert_eq!(stack.drawn(), 3);
    stack.update(&entries, 40, Detail::Preview, &BTreeSet::new(), &[]);
    assert_eq!(stack.drawn(), 0);
}

#[test]
fn a_growing_answer_draws_only_itself_again() {
    let mut entries = vec![user("one"), answer("tw")];
    let mut stack = stacked(&entries);
    entries[1] = answer("two, and more");
    stack.update(&entries, 40, Detail::Preview, &BTreeSet::new(), &[]);
    assert_eq!(stack.drawn(), 1);
    let shown: String = stack
        .rows(0..stack.len())
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
        .collect();
    assert!(shown.contains("two, and more"), "{shown}");
}

#[test]
fn a_new_width_draws_everything_again() {
    let entries = [user("one"), call("t1")];
    let mut stack = stacked(&entries);
    stack.update(&entries, 60, Detail::Preview, &BTreeSet::new(), &[]);
    assert_eq!(stack.drawn(), 2);
}

#[test]
fn flipping_one_call_draws_only_that_call_again() {
    let entries = [call("t1"), call("t2")];
    let mut stack = stacked(&entries);
    let flipped: BTreeSet<ToolCallId> = [ToolCallId::new("t2")].into();
    stack.update(&entries, 40, Detail::Preview, &flipped, &[]);
    assert_eq!(stack.drawn(), 1);
}

#[test]
fn the_stack_lays_each_block_out_with_one_gap_between() {
    let entries = [user("one"), call("t1")];
    let stack = stacked(&entries);
    let first = entry_lines(&entries[0], 40, Detail::Preview);
    let second = entry_lines(&entries[1], 40, Detail::Preview);
    assert_eq!(stack.len(), first.len() + 1 + second.len());
    assert_eq!(stack.at(first.len()), None, "the gap belongs to no block");
    assert_eq!(stack.at(first.len() + 1), Some((1, 0)), "the call's header");
    assert_eq!(
        stack.block(1).map(|(start, _)| start),
        Some(first.len() + 1)
    );
}

#[test]
fn an_entry_that_draws_nothing_takes_no_row_and_no_gap() {
    let entries = [
        user("one"),
        Entry::Masked {
            id: MessageId::new("m"),
            at: 0,
            shown: String::new(),
        },
        user("two"),
    ];
    let stack = stacked(&entries);
    let one = entry_lines(&entries[0], 40, Detail::Preview).len();
    let two = entry_lines(&entries[2], 40, Detail::Preview).len();
    assert_eq!(stack.len(), one + 1 + two);
    assert_eq!(stack.at(one + 1), Some((2, 0)));
}

#[test]
fn a_row_past_the_end_is_nobodys() {
    let stack = stacked(&[user("one")]);
    assert_eq!(stack.at(stack.len()), None);
    assert!(stack.line(stack.len()).is_none());
}

#[test]
fn a_shorter_transcript_drops_what_it_no_longer_has() {
    let mut stack = stacked(&[user("one"), user("two")]);
    stack.update(&[], 40, Detail::Preview, &BTreeSet::new(), &[]);
    assert!(stack.is_empty());
    assert_eq!(stack.at(0), None);
}

#[test]
fn a_call_is_found_by_its_id() {
    let stack = stacked(&[user("one"), call("t1"), call("t2")]);
    assert_eq!(stack.find(&ToolCallId::new("t2")), Some(2));
    assert_eq!(stack.call(2), Some(&ToolCallId::new("t2")));
    assert_eq!(stack.call(0), None);
}

fn frame(text: &str) -> Vec<Vec<Span>> {
    vec![vec![Span::new(magi_proto::tooling::Role::Text, text)]]
}

fn text(stack: &Stack) -> Vec<String> {
    stack
        .rows(0..stack.len())
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn live<'a>(id: &'a ToolCallId, drawn: &'a [Vec<Span>]) -> Live<'a> {
    Live {
        call: id,
        tool: "dino",
        title: "a game",
        drawn,
        away: None,
        pin: None,
        focused: false,
    }
}

#[test]
fn a_live_call_draws_its_frame_in_its_own_block() {
    let entries = [user("play"), call("t1")];
    let id = ToolCallId::new("t1");
    let drawn = frame("the dinosaur runs");
    let mut stack = Stack::new();
    stack.update(
        &entries,
        40,
        Detail::Preview,
        &BTreeSet::new(),
        &[live(&id, &drawn)],
    );
    let rows = text(&stack);
    let header = rows
        .iter()
        .position(|row| row.contains("[ dino ]"))
        .expect("the live block's header");
    assert!(rows[header + 1].contains("the dinosaur runs"), "{rows:#?}");
    assert!(
        !rows[header].contains(crate::glyph::expand()),
        "nothing to fold: {rows:#?}"
    );
    assert!(
        rows.iter()
            .filter(|row| !row.is_empty())
            .all(|row| crate::wrap::columns(row) == 40),
        "{rows:#?}"
    );
}

#[test]
fn a_new_frame_draws_only_its_block_again() {
    let entries = [user("play"), call("t1")];
    let id = ToolCallId::new("t1");
    let (one, two) = (frame("one"), frame("two"));
    let mut stack = Stack::new();
    stack.update(
        &entries,
        40,
        Detail::Preview,
        &BTreeSet::new(),
        &[live(&id, &one)],
    );
    stack.update(
        &entries,
        40,
        Detail::Preview,
        &BTreeSet::new(),
        &[live(&id, &one)],
    );
    assert_eq!(stack.drawn(), 0, "the same frame twice");
    stack.update(
        &entries,
        40,
        Detail::Preview,
        &BTreeSet::new(),
        &[live(&id, &two)],
    );
    assert_eq!(stack.drawn(), 1);
    assert!(text(&stack).iter().any(|row| row.contains("two")));
}

#[test]
fn before_its_first_frame_a_live_block_says_what_it_is_for() {
    let entries = [call("t1")];
    let id = ToolCallId::new("t1");
    let mut stack = Stack::new();
    stack.update(
        &entries,
        40,
        Detail::Preview,
        &BTreeSet::new(),
        &[live(&id, &[])],
    );
    assert!(text(&stack).iter().any(|row| row.contains("a game")));
}

#[test]
fn when_the_tool_lets_go_the_block_is_the_call_again() {
    let entries = [call("t1")];
    let id = ToolCallId::new("t1");
    let drawn = frame("the dinosaur runs");
    let mut stack = Stack::new();
    stack.update(
        &entries,
        40,
        Detail::Preview,
        &BTreeSet::new(),
        &[live(&id, &drawn)],
    );
    stack.update(&entries, 40, Detail::Preview, &BTreeSet::new(), &[]);
    assert_eq!(stack.drawn(), 1);
    let rows = text(&stack);
    assert!(rows[0].contains("[ shell ]"), "{rows:#?}");
    assert!(
        !rows.iter().any(|row| row.contains("dinosaur")),
        "{rows:#?}"
    );
}

#[test]
fn a_frame_wider_than_the_block_is_cut_at_its_edge() {
    let entries = [call("t1")];
    let id = ToolCallId::new("t1");
    let drawn = frame(&"x".repeat(200));
    let mut stack = Stack::new();
    stack.update(
        &entries,
        40,
        Detail::Preview,
        &BTreeSet::new(),
        &[live(&id, &drawn)],
    );
    assert!(
        text(&stack)
            .iter()
            .all(|row| crate::wrap::columns(row) == 40),
        "{:#?}",
        text(&stack)
    );
}
