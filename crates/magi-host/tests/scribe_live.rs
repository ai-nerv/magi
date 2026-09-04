//! The scrollback contract, against a balthasar that is actually running.
//!
//! Skipped when there is no socket. What these prove cannot be proved against a mock: that an
//! `Entry` handed to balthasar comes back the same `Entry`, including the fields no projection
//! carries.

use magi_host::scribe::Scribe;
use magi_ipc::family::Family;
use magi_proto::{Cursor, Entry, MessageId, SessionId, StopReason, ToolCallId, ToolResult, Usage};

async fn scribe(name: &str) -> Option<Scribe> {
    let path = magi_ipc::family::candidates(None).into_iter().next()?;
    let family = match Family::dial(&path).await {
        Ok(open) => open,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let id = SessionId::new(format!("magi-scribe-{}-{name}", std::process::id()));
    Some(Scribe::over(family, None, &id))
}

#[tokio::test]
async fn an_entry_survives_the_round_trip_unaltered() {
    let Some(mut scribe) = scribe("plain").await else {
        return;
    };
    let entry = Entry::User {
        id: MessageId::new("u1"),
        text: "what did we decide".into(),
        aside: "context nobody is shown".into(),
    };

    scribe.observe(Cursor(1), &entry).await.expect("observe");
    let back = scribe.replay().await.expect("replay");

    assert_eq!(back.len(), 1, "one turn in, one turn out: {back:?}");
    assert_eq!(back[0], (Cursor(1), entry), "the entry changed on the wire");
}

#[tokio::test]
async fn the_fields_no_projection_carries_come_back() {
    let Some(mut scribe) = scribe("opaque").await else {
        return;
    };
    // Every field that cannot be recomputed: a provider signature, a usage count, an error and
    // the stop reason that explains it.
    let entry = Entry::Assistant {
        id: MessageId::new("a1"),
        text: "partial".into(),
        thinking: "reasoning".into(),
        stop_reason: Some(StopReason::Error),
        error: Some("the provider hung up".into()),
        signatures: Default::default(),
        usage: Usage {
            input: 11,
            output: 22,
            ..Default::default()
        },
    };

    scribe.observe(Cursor(1), &entry).await.expect("observe");
    let back = scribe.replay().await.expect("replay");
    assert_eq!(back[0].1, entry, "an unrecomputable field was lost");
}

#[tokio::test]
async fn a_tool_signature_is_not_flattened_into_the_projection() {
    let Some(mut scribe) = scribe("signature").await else {
        return;
    };
    let entry = Entry::Tool {
        id: ToolCallId::new("t1"),
        name: "shell".into(),
        args: "{\"command\":\"ls\"}".into(),
        result: Some(ToolResult {
            output: "a\nb\n".into(),
            is_error: false,
            shown: None,
        }),
        thought_signature: Some("opaque-provider-state".into()),
    };

    scribe.observe(Cursor(1), &entry).await.expect("observe");
    assert_eq!(scribe.replay().await.expect("replay")[0].1, entry);
}

#[tokio::test]
async fn amending_replaces_the_turn_rather_than_appending_one() {
    let Some(mut scribe) = scribe("amend").await else {
        return;
    };
    let growing = |text: &str| Entry::Assistant {
        id: MessageId::new("a1"),
        text: text.into(),
        thinking: String::new(),
        stop_reason: None,
        error: None,
        signatures: Default::default(),
        usage: Usage::default(),
    };

    scribe
        .observe(Cursor(1), &growing("par"))
        .await
        .expect("observe");
    scribe
        .amend(Cursor(1), &growing("partial"))
        .await
        .expect("amend");
    scribe
        .amend(Cursor(1), &growing("partial answer"))
        .await
        .expect("amend");

    let back = scribe.replay().await.expect("replay");
    assert_eq!(
        back.len(),
        1,
        "amend appended instead of replacing: {back:?}"
    );
    assert_eq!(back[0].1, growing("partial answer"), "not the final state");
}

#[tokio::test]
async fn cursor_order_is_what_comes_back_not_arrival_order() {
    let Some(mut scribe) = scribe("order").await else {
        return;
    };
    let at = |n: u64| Entry::User {
        id: MessageId::new(format!("u{n}")),
        text: format!("message {n}"),
        aside: String::new(),
    };

    // Written out of order on purpose.
    scribe.observe(Cursor(3), &at(3)).await.expect("observe 3");
    scribe.observe(Cursor(1), &at(1)).await.expect("observe 1");
    scribe.observe(Cursor(2), &at(2)).await.expect("observe 2");

    let back = scribe.replay().await.expect("replay");
    let cursors: Vec<u64> = back.iter().map(|(c, _)| c.0).collect();
    assert_eq!(cursors, vec![1, 2, 3], "replay must be in cursor order");
}

/// The path a turn actually takes: commit into a session, then flush it out.
#[tokio::test]
async fn what_a_session_commits_reaches_balthasar_when_it_is_flushed() {
    let Some(scribe) = scribe("flush").await else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("magi-flush-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let journal = dir.join("session.jsonl");
    let id = SessionId::new(format!("magi-scribe-{}-flush", std::process::id()));

    let session = magi_host::session::Session::open(&journal, id, "/tmp", 1).expect("open");
    let session = tokio::sync::Mutex::new(session);

    {
        let mut held = session.lock().await;
        held.commit(Entry::User {
            id: MessageId::new("u1"),
            text: "asked".into(),
            aside: String::new(),
        })
        .expect("commit");
        // Amended twice, as a streaming message is. Both collapse into one write.
        held.commit(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "par".into(),
            thinking: String::new(),
            stop_reason: None,
            error: None,
            signatures: Default::default(),
            usage: Usage::default(),
        })
        .expect("commit");
        held.amend(Entry::Assistant {
            id: MessageId::new("a1"),
            text: "partial answer".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
            signatures: Default::default(),
            usage: Usage::default(),
        })
        .expect("amend");
        assert!(held.has_pending(), "commits must queue for balthasar");
    }

    let mut scribe = Some(scribe);
    magi_host::scribe::flush(&session, &mut scribe)
        .await
        .expect("flush");
    assert!(!session.lock().await.has_pending(), "the queue must drain");

    let back = scribe
        .as_mut()
        .expect("scribe")
        .replay()
        .await
        .expect("replay");
    assert_eq!(back.len(), 2, "two entries, not three: {back:?}");
    assert_eq!(
        back[1].1,
        Entry::Assistant {
            id: MessageId::new("a1"),
            text: "partial answer".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::EndTurn),
            error: None,
            signatures: Default::default(),
            usage: Usage::default(),
        },
        "the amendment must win"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
