use super::*;
use magi_proto::{HarnessEvent, MessageId, ModelInfo, StopReason, Usage};

#[test]
fn pending_weight_counts_user_and_tool_image_payloads_and_media_types() {
    let images = vec![magi_model::Image {
        data: "AAAA".repeat(100),
        media_type: "image/png".into(),
    }];
    let bytes = images[0].data.len() + images[0].media_type.len();
    let user = Entry::User {
        id: MessageId::new("u"),
        text: "prompt".into(),
        aside: "aside".into(),
        images: images.clone(),
    };
    assert_eq!(weight(&user), 11 + bytes);
    let tool = Entry::Tool {
        id: magi_proto::ToolCallId::new("t"),
        name: "image".into(),
        args: "{}".into(),
        thought_signature: None,
        result: Some(magi_proto::ToolResult {
            output: "pixels".into(),
            images,
            is_error: false,
            shown: None,
        }),
    };
    assert_eq!(weight(&tool), 8 + bytes);
}

#[test]
fn pending_opaque_payloads_count_all_carriers_and_trigger_byte_eviction() {
    for carrier in 0..3 {
        let opaque = "x".repeat(1024 * 1024);
        let entry = if carrier < 2 {
            Entry::Assistant {
                id: MessageId::new("signed"),
                text: String::new(),
                thinking: String::new(),
                stop_reason: Some(StopReason::EndTurn),
                error: None,
                usage: Usage::default(),
                signatures: magi_proto::Signatures {
                    text: (carrier == 0).then(|| opaque.clone()),
                    thinking: (carrier == 1).then(|| opaque.clone()),
                },
            }
        } else {
            Entry::Tool {
                id: magi_proto::ToolCallId::new("signed"),
                name: "echo".into(),
                args: String::new(),
                result: None,
                thought_signature: Some(opaque.clone()),
            }
        };
        assert_eq!(weight(&entry), opaque.len(), "carrier {carrier}");
        let count = PENDING_BYTES / weight(&entry) + 1;
        assert!(count < PENDING_MOST);
        let mut session = Session::recorded(SessionId::new("opaque"), vec![]);
        for cursor in 1..=count {
            session.pending.insert(cursor as u64, entry.clone());
        }
        let warning = session.bound_pending().expect("byte-bound eviction");
        assert!(warning.contains("1 of the oldest") && warning.contains("cursors 1 to 1"));
        assert_eq!(session.pending.len(), count - 1);
        assert!(session.pending.values().map(weight).sum::<usize>() <= PENDING_BYTES);
    }
}

#[test]
fn pending_image_payloads_trigger_byte_eviction_before_the_entry_count_limit() {
    for tool in [false, true] {
        let images = vec![
            magi_model::Image {
                data: "AAAA".repeat(16 * 1024),
                media_type: "image/png".into()
            };
            4
        ];
        let entry = if tool {
            Entry::Tool {
                id: magi_proto::ToolCallId::new("t"),
                name: "paint".into(),
                args: "{}".into(),
                thought_signature: None,
                result: Some(magi_proto::ToolResult {
                    output: "pixels".into(),
                    images,
                    is_error: false,
                    shown: None,
                }),
            }
        } else {
            Entry::User {
                id: MessageId::new("u"),
                text: "prompt".into(),
                aside: String::new(),
                images,
            }
        };
        let count = PENDING_BYTES / weight(&entry) + 1;
        assert!(count < PENDING_MOST);
        let mut session = Session::recorded(SessionId::new("images"), vec![]);
        for cursor in 1..=count {
            session.pending.insert(cursor as u64, entry.clone());
        }
        let warning = session.bound_pending().expect("byte-bound eviction");
        assert!(warning.contains("1 of the oldest") && warning.contains("cursors 1 to 1"));
        assert_eq!(session.pending.len(), count - 1);
        assert!(!session.pending.contains_key(&1));
        assert!(session.pending.values().map(weight).sum::<usize>() <= PENDING_BYTES);
    }
}

fn answer(id: &str, output: u64) -> Entry {
    Entry::Assistant {
        id: MessageId::new(id),
        text: id.into(),
        thinking: String::new(),
        stop_reason: Some(StopReason::EndTurn),
        error: None,
        signatures: Default::default(),
        usage: Usage {
            output,
            ..Default::default()
        },
    }
}

#[test]
fn replacement_resets_session_state_but_preserves_live_subscriptions() {
    let mut session = Session::recorded(SessionId::new("A"), vec![answer("A", 9)]);
    let model = ModelInfo {
        name: "fake/one".into(),
        context_window: 1000,
    };
    session.set_model(Some(model.clone()));
    session.hints.insert("A-tool".into(), Default::default());
    session.laid = Some(
        serde_json::from_value(serde_json::json!({"id":"A-layout", "slots":[]})).expect("layout"),
    );
    session.rest();
    session.deferred.push(
        serde_json::from_value(
            serde_json::json!({"id":"A-job", "role":"memory", "prompt":"old", "when":"later"}),
        )
        .expect("job"),
    );
    let old_spend = session.helpers_spent();
    old_spend.store(99, std::sync::atomic::Ordering::SeqCst);
    let old_cancel = session.cancel();
    old_cancel.request();
    let old_helpers = session.helpers();
    let paused = old_helpers.pause().expect("pause");
    let mut first = session.subscribe();
    let mut second = session.subscribe();
    let phase = session.phase_watch();
    let spending = session.spent_watch();
    let journal =
        Journal::restore(SessionId::new("B"), vec![(Cursor(7), answer("B", 3))]).expect("B");
    paused.retire();
    session.resume_prepared(journal);
    for reader in [&mut first, &mut second] {
        assert!(
            matches!(reader.try_recv().expect("snapshot"), HarnessEvent::SessionSnapshot { session, cursor: Cursor(7), entries, status: AgentStatus::Idle, .. } if session.as_str() == "B" && entries == vec![answer("B", 3)])
        );
        assert!(reader.try_recv().is_err());
    }
    assert_eq!(*phase.borrow(), AgentStatus::Idle);
    assert_eq!(spending.borrow()[0].1.output, 3);
    assert_eq!(session.model(), Some(model));
    assert!(
        session.hints.is_empty()
            && session.laid.is_none()
            && session.rested.is_none()
            && session.deferred.is_empty()
    );
    assert!(session.counted.contains(&MessageId::new("B")));
    assert!(!session.counted.contains(&MessageId::new("A")));
    assert!(!session.has_pending());
    assert!(!session.cancel().is_requested());
    assert!(old_cancel.is_requested());
    assert!(!std::sync::Arc::ptr_eq(
        &old_spend,
        &session.helpers_spent()
    ));
    assert_eq!(
        session
            .helpers_spent()
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(old_helpers.pause().is_err());
    assert!(session.helpers().pause().is_ok());
}

#[test]
fn acknowledging_an_old_version_does_not_clear_a_newer_revision() {
    let mut session = Session::recorded(SessionId::new("A"), Vec::new());
    let cursor = session.commit(answer("same", 1)).expect("commit");
    session.amend_at(cursor, answer("same", 2)).expect("amend");
    session.acknowledge_pending(cursor, &answer("same", 1));
    assert_eq!(session.pending_batch(), vec![(cursor, answer("same", 2))]);
    session.acknowledge_pending(cursor, &answer("same", 2));
    assert!(!session.has_pending());
}

#[test]
fn sparse_replay_and_snapshot_use_stored_cursors_not_offsets() {
    let session = Session::restored(
        SessionId::new("B"),
        vec![
            (Cursor(7), answer("one", 1)),
            (Cursor(13), answer("two", 2)),
        ],
    )
    .expect("restore");
    assert!(
        matches!(session.snapshot(Cursor(10)), HarnessEvent::SessionSnapshot { entries, .. } if entries == vec![answer("one", 1)])
    );
    let events = session.replay(Cursor(10));
    assert!(events.iter().all(|e| e.cursor() == Cursor(13)));
    assert!(!events.is_empty());
    assert!(session.replay(Cursor(13)).is_empty());
}
