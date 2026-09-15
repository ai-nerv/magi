use super::*;
use magi_model::{Content, Role};
use magi_proto::{MessageId, Signatures, ToolCallId, ToolResult};

fn user(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(text),
        text: text.into(),
        aside: String::new(),
    }
}

fn said(text: &str) -> Entry {
    Entry::Assistant {
        id: MessageId::new(text),
        text: text.into(),
        thinking: String::new(),
        stop_reason: Some(magi_model::StopReason::ToolUse),
        error: None,
        signatures: Signatures::default(),
        usage: magi_proto::Usage::default(),
    }
}

fn tool(id: &str, output: &str) -> Entry {
    Entry::Tool {
        id: ToolCallId::new(id),
        name: "read".into(),
        args: "{}".into(),
        result: Some(ToolResult {
            output: output.into(),
            is_error: false,
            shown: None,
        }),
        thought_signature: None,
    }
}

/// Cursors 1..=5: a question, a call with its result, an answer, and the next question.
fn transcript() -> Vec<Entry> {
    vec![
        user("first"),
        said("reading"),
        tool("c1", "the whole file"),
        said("done"),
        user("second"),
    ]
}

fn layout(slots: serde_json::Value) -> Layout {
    serde_json::from_value(serde_json::json!({ "id": "L-1", "slots": slots })).expect("a layout")
}

fn texts(context: &Context) -> Vec<String> {
    context
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            Content::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_stub_replaces_the_result_and_keeps_the_call() {
    let laid = layout(serde_json::json!([
        {"kind":"item","cursor":1}, {"kind":"item","cursor":2},
        {"kind":"stub","cursor":3,"text":"read a.rs (400 lines)"},
        {"kind":"item","cursor":4}, {"kind":"item","cursor":5},
    ]));
    let context = render(&transcript(), &laid);
    let shown = texts(&context);
    assert!(
        shown.contains(&"read a.rs (400 lines)".to_owned()),
        "{shown:?}"
    );
    assert!(!shown.iter().any(|t| t == "the whole file"), "{shown:?}");
    assert!(
        context.messages.iter().any(|m| m
            .content
            .iter()
            .any(|c| matches!(c, Content::ToolCall { .. }))),
        "the call a stubbed result answers is still sent"
    );
}

#[test]
fn text_slots_become_user_messages_where_balthasar_put_them() {
    let laid = layout(serde_json::json!([
        {"kind":"pinned","text":"always use make"},
        {"kind":"summary","text":"they read a file","covers":[1,4]},
        {"kind":"memory","text":"a.rs is generated","injection":"inject-9"},
        {"kind":"item","cursor":5},
    ]));
    let context = render(&transcript(), &laid);
    let roles: Vec<Role> = context.messages.iter().map(|m| m.role).collect();
    assert!(roles.iter().all(|r| *r == Role::User), "{roles:?}");
    let shown = texts(&context).join("\n");
    let pinned = shown.find("always use make").expect("pinned");
    let summary = shown.find("they read a file").expect("summary");
    let memory = shown.find("a.rs is generated").expect("memory");
    let prompt = shown.find("second").expect("the prompt");
    assert!(
        pinned < summary && summary < memory && memory < prompt,
        "{shown}"
    );
}

#[test]
fn what_no_slot_names_is_dropped_and_counted() {
    let laid = layout(serde_json::json!([
        {"kind":"summary","text":"earlier"}, {"kind":"item","cursor":5},
    ]));
    let live = live(&transcript());
    let counted = counts(&laid, &live);
    assert_eq!(counted.items, 1);
    assert_eq!(counted.dropped, 4);
    assert_eq!(counted.summary, 1);
}

#[test]
fn a_slot_of_a_kind_this_build_does_not_know_is_skipped_not_fatal() {
    let laid = layout(serde_json::json!([
        {"kind":"hologram","text":"?"}, {"kind":"item","cursor":5},
    ]));
    assert_eq!(laid.slots[0], Slot::Other);
    assert_eq!(
        texts(&render(&transcript(), &laid)),
        vec!["second".to_owned()]
    );
}

#[test]
fn the_last_layout_grows_by_what_is_new_and_forgets_its_id() {
    let last = layout(serde_json::json!([
        {"kind":"summary","text":"earlier"},
        {"kind":"stub","cursor":3,"text":"stub"},
        {"kind":"item","cursor":4},
    ]));
    let grown = extend(&last, &[1, 2, 3, 4, 5, 6]);
    assert!(grown.id.is_empty(), "nothing to report against");
    let named: Vec<u64> = grown.slots.iter().filter_map(Slot::cursor).collect();
    assert_eq!(
        named,
        [3, 4, 5, 6],
        "dropped stays dropped, what is new is sent"
    );
    assert!(
        matches!(grown.slots[1], Slot::Stub { .. }),
        "a stub stays a stub"
    );
}

#[test]
fn a_layout_that_leaves_out_the_prompt_is_not_sent() {
    let live = live(&transcript());
    assert!(!sound(
        &layout(serde_json::json!([{"kind":"item","cursor":1}])),
        &live
    ));
    assert!(sound(
        &layout(serde_json::json!([{"kind":"item","cursor":5}])),
        &live
    ));
}

#[test]
fn errors_and_notices_are_never_offered() {
    let mut entries = transcript();
    entries.push(Entry::Notice {
        text: "a UI talking".into(),
    });
    entries.push(Entry::Assistant {
        id: MessageId::new("bad"),
        text: String::new(),
        thinking: String::new(),
        stop_reason: Some(magi_model::StopReason::Error),
        error: Some("402".into()),
        signatures: Signatures::default(),
        usage: magi_proto::Usage::default(),
    });
    assert_eq!(live(&entries), [1, 2, 3, 4, 5]);
}

#[test]
fn everything_live_is_what_goes_when_there_was_never_a_layout() {
    let entries = transcript();
    let everything = render(&entries, &whole(&live(&entries)));
    assert_eq!(everything, crate::context::of_entries(&entries));
}

#[test]
fn a_plan_from_a_memory_that_does_not_lay_out_is_sent_as_one() {
    let plan = serde_json::json!({
        "mask": [{ "cursor": 3, "as": "read a.rs (400 lines)" }],
        "drop": [1],
        "why": "over budget",
    });
    let laid = from_plan(&plan, &[1, 2, 3, 4, 5]);
    assert!(
        laid.id.is_empty(),
        "a plan has nothing to report back against"
    );
    let named: Vec<u64> = laid.slots.iter().filter_map(Slot::cursor).collect();
    assert_eq!(named, [2, 3, 4, 5], "what it drops is left out");
    assert!(matches!(&laid.slots[1], Slot::Stub { text, .. } if text == "read a.rs (400 lines)"));
    assert!(laid.why.contains("over budget"), "{}", laid.why);
}

#[test]
fn every_slot_is_listed_with_what_it_costs() {
    let laid = layout(serde_json::json!([
        {"kind":"summary","text":"they read a file","tokens":900},
        {"kind":"stub","cursor":3,"text":"read a.rs (400 lines)"},
        {"kind":"item","cursor":5},
    ]));
    let listed = listed(&transcript(), &laid);
    let kinds: Vec<&str> = listed.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["summary", "stub", "item"]);
    assert_eq!(
        listed[0].tokens, 900,
        "balthasar's own count, when it gave one"
    );
    assert_eq!(listed[1].cursor, Some(3));
    assert_eq!(listed[2].text, "you: second");
}

/// The rule that keeps a helper off the person's time: a layout's background jobs are held for after
/// the turn, and nothing is run or spent while the request is being built.
#[tokio::test]
async fn background_jobs_are_held_for_after_the_turn_not_run_inside_it() {
    let session = tokio::sync::Mutex::new(Session::recorded(
        magi_proto::SessionId::new("s"),
        transcript(),
    ));
    let mut heard = session.lock().await.subscribe();
    let mut laid = layout(serde_json::json!([{"kind":"item","cursor":5}]));
    laid.jobs = vec![crate::helping::Job {
        id: "J-1".into(),
        kind: "summarise".into(),
        ..Default::default()
    }];
    let mut prompt = Prompt::default();
    settle(&session, Some(laid), &mut prompt).await;

    let deferred = session.lock().await.take_deferred();
    assert_eq!(deferred.len(), 1, "the job waits for the turn to end");
    let mut spent = false;
    let mut reported = None;
    while let Ok(event) = heard.try_recv() {
        match event {
            HarnessEvent::HelperSpent { .. } => spent = true,
            HarnessEvent::ContextLaid { slots, .. } => reported = Some(slots),
            _ => {}
        }
    }
    assert!(!spent, "nothing ran while the request was built");
    assert_eq!(
        reported.map(|s| s.len()),
        Some(1),
        "the slots go to the screen"
    );
}
