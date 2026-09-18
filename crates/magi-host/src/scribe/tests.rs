use super::*;
use magi_proto::{MessageId, ToolCallId, ToolResult};

/// Both shapes balthasar answers `recall` in. The setting that decides is balthasar's, and both are
/// the ordinary case on somebody's machine.
#[test]
fn a_recall_with_no_ledger_is_a_list_of_memories() {
    let answered = Recalled::of(&[serde_json::json!([
        { "id": "m1", "text": "one" },
        { "id": "m2", "text": "two" },
    ])]);
    assert_eq!(answered.memories.len(), 2);
    assert_eq!(answered.injection, None, "there is no ledger to belong to");
}

#[test]
fn a_recall_with_a_ledger_carries_the_id_that_makes_an_outcome_attributable() {
    let answered = Recalled::of(&[serde_json::json!({
        "injection": "inject-1700-abc",
        "memories": [{ "id": "m1", "text": "one" }],
    })]);
    assert_eq!(answered.memories.len(), 1);
    assert_eq!(answered.injection.as_deref(), Some("inject-1700-abc"));
}

#[test]
fn a_recall_that_found_nothing_is_neither() {
    assert_eq!(Recalled::of(&[]), Recalled::default());
    assert!(Recalled::of(&[serde_json::json!([])]).memories.is_empty());
}

fn assistant(text: &str, thinking: &str) -> Entry {
    Entry::Assistant {
        id: MessageId::new("a1"),
        text: text.into(),
        thinking: thinking.into(),
        stop_reason: None,
        error: None,
        signatures: Default::default(),
        usage: Default::default(),
    }
}

fn read(output: &str, failed: bool) -> Entry {
    Entry::Tool {
        id: ToolCallId::new("t1"),
        name: "read".into(),
        args: "{}".into(),
        result: Some(ToolResult {
            output: output.into(),
            is_error: failed,
            shown: None,
        }),
        thought_signature: None,
    }
}

#[test]
fn every_variant_gets_a_kind_of_its_own_where_it_needs_one() {
    let user = Entry::User {
        id: MessageId::new("u1"),
        text: "hi".into(),
        aside: String::new(),
    };
    let from = Entry::From {
        who: "p/x".into(),
        kin: "sibling".into(),
        sort: "question".into(),
        text: "hi".into(),
    };
    assert_eq!(kind(&user), "user");
    assert_eq!(kind(&from), "from");
    assert_ne!(kind(&user), kind(&from), "a sibling is not the person");
}

#[test]
fn a_tool_changes_kind_when_its_result_lands() {
    let mut call = Entry::Tool {
        id: ToolCallId::new("t1"),
        name: "shell".into(),
        args: "{}".into(),
        result: None,
        thought_signature: None,
    };
    assert_eq!(kind(&call), "tool_call");
    if let Entry::Tool { result, .. } = &mut call {
        *result = Some(ToolResult {
            output: "done".into(),
            is_error: false,
            shown: None,
        });
    }
    assert_eq!(kind(&call), "tool_result");
}

#[test]
fn a_message_that_is_only_reasoning_is_thinking_rather_than_prose() {
    assert_eq!(kind(&assistant("", "mulling")), "thinking");
    assert_eq!(kind(&assistant("said", "mulling")), "prose");
}

#[test]
fn the_raw_record_is_what_travels_and_it_round_trips() {
    let entry = assistant("said", "mulling");
    let wire = turn(Cursor(7), &entry, &Beside::default()).expect("turn");
    assert_eq!(wire["cursor"], serde_json::json!(7));

    let (cursor, back) = rebuild(&wire).expect("rebuild");
    assert_eq!(cursor, Cursor(7));
    assert_eq!(back, entry, "the entry must survive the wire unaltered");
}

#[test]
fn a_row_whose_raw_is_a_string_rebuilds_the_same_as_one_that_is_an_object() {
    let entry = assistant("said", "");
    let wire = turn(Cursor(2), &entry, &Beside::default()).expect("turn");
    let as_text = serde_json::json!({
        "raw": serde_json::to_string(&wire["raw"]).expect("stringify"),
    });
    assert_eq!(rebuild(&as_text).expect("rebuild"), (Cursor(2), entry));
}

#[test]
fn a_row_with_no_raw_is_malformed_rather_than_an_empty_entry() {
    let row = serde_json::json!({ "cursor": 1, "text": "hi", "kind": "user" });
    assert!(matches!(rebuild(&row), Err(Fault::Malformed(_))));
}

#[test]
fn the_projection_never_stands_in_for_the_record() {
    // A signature is in `raw` and nowhere else; rebuilding from `text` would be a 400.
    let entry = Entry::Tool {
        id: ToolCallId::new("t1"),
        name: "shell".into(),
        args: "{\"command\":\"ls\"}".into(),
        result: None,
        thought_signature: Some("opaque-signature".into()),
    };
    let wire = turn(Cursor(3), &entry, &Beside::default()).expect("turn");
    let shown = wire["text"].as_str().expect("text is a string");
    assert!(
        !shown.contains("opaque-signature"),
        "the signature leaked into the projection"
    );
    let (_, back) = rebuild(&wire).expect("rebuild");
    assert_eq!(back, entry);
}

#[test]
fn a_tool_row_carries_its_group_its_cost_and_what_its_tool_said() {
    let beside = Beside {
        group: Some(4),
        hints: magi_proto::tooling::Hints {
            brief: Some("read a (1 line)".into()),
            back: Some("read a".into()),
            keep: false,
        },
    };
    let wire = turn(Cursor(5), &read("12345678", false), &beside).expect("turn");
    assert_eq!(wire["group"], 4);
    assert_eq!(wire["stub"], "read a (1 line)");
    assert_eq!(wire["handle"], "read a");
    assert_eq!(
        wire["args"],
        serde_json::json!({}),
        "as JSON, not as a string holding some"
    );
    // A word, two marks and eight digits in groups of three: one, two and three.
    assert_eq!(wire["tokens"], 6);
    assert_eq!(wire["keep"], false);
    assert_eq!(wire["error"], false);
}

#[test]
fn a_failed_tool_is_kept_whatever_its_tool_said() {
    let wire = turn(Cursor(5), &read("no such file", true), &Beside::default()).expect("turn");
    assert_eq!(wire["error"], true);
    assert_eq!(wire["keep"], true);
}

#[test]
fn a_tool_belongs_to_the_message_that_called_it() {
    let mut session =
        crate::session::Session::recorded(magi_proto::SessionId::new("s"), Vec::new());
    session
        .commit(Entry::User {
            id: MessageId::new("u1"),
            text: "go".into(),
            aside: String::new(),
        })
        .expect("commit");
    let asked = session.commit(assistant("", "")).expect("commit");
    let called = session.commit(read("x", false)).expect("commit");
    session.hint(
        "t1",
        magi_proto::tooling::Hints {
            brief: Some("read x".into()),
            ..Default::default()
        },
    );
    assert_eq!(beside(&session, asked, &assistant("", "")).group, Some(2));
    let tool = beside(&session, called, &read("x", false));
    assert_eq!(tool.group, Some(2), "the assistant at cursor 2 called it");
    assert_eq!(tool.hints.brief.as_deref(), Some("read x"));
}
