use super::{backend, session, turn};
use magi_model::Content;
use magi_proto::{Entry, HarnessEvent, MessageId, ToolCallId, ToolResult, caching::Invalidation};
use magi_testkit::{
    Mind,
    mind::{stop_line, text_line},
};
use serde_json::json;

fn user(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(text),
        text: text.into(),
        aside: String::new(),
        images: vec![],
    }
}

fn frames(events: &mut tokio::sync::broadcast::Receiver<HarnessEvent>) -> Vec<Vec<Invalidation>> {
    let mut frames = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let HarnessEvent::CacheFrame { reasons } = event {
            frames.push(reasons);
        }
    }
    frames
}

#[tokio::test]
async fn actual_asks_preserve_signed_history_across_rules_model_and_preference_changes() {
    let (session, _dir) = session("frame-transitions");
    let mind = Mind::saying(
        "frame-transitions",
        &[
            &json!({"event":"thinking","text":"opaque reasoning"}).to_string(),
            &json!({"event":"signature","signature":"unaltered signature"}).to_string(),
            &text_line("answer"),
            &stop_line(),
        ],
    );
    let mut backend = backend(&mind);
    backend.system = Some("original rules".into());
    let mut events = session.lock().await.subscribe();
    for (nth, expected) in [
        vec![],
        vec![],
        vec![Invalidation::RulesChanged],
        vec![Invalidation::ModelChanged],
        vec![Invalidation::OptionsChanged],
    ]
    .into_iter()
    .enumerate()
    {
        match nth {
            2 => backend.system = Some("fresh rules".into()),
            3 => backend.model = "fake/two".into(),
            4 => backend.wants.provider = Some("explicit".into()),
            _ => {}
        }
        session
            .lock()
            .await
            .commit(user(&format!("question {nth}")))
            .expect("question");
        turn(&session, &backend).await;
        assert_eq!(frames(&mut events), [expected]);
    }
    let asks = mind
        .asks()
        .iter()
        .map(|ask| serde_json::from_str::<magi_proto::ask::Ask>(ask).expect("ask"))
        .collect::<Vec<_>>();
    assert_eq!(asks.len(), 5);
    for pair in asks.windows(2) {
        assert!(
            pair[1]
                .context
                .messages
                .starts_with(&pair[0].context.messages)
        );
    }
    assert_eq!(asks[2].context.system.as_deref(), Some("fresh rules"));
    assert_eq!(asks[3].model, "fake/two");
    assert_eq!(asks[4].wants.provider.as_deref(), Some("explicit"));
    assert!(asks[4].context.messages.iter().flat_map(|m| &m.content).any(|c| matches!(c,
        Content::Thinking { thinking, signature:Some(signature) } if thinking=="opaque reasoning" && signature=="unaltered signature")));
}

#[tokio::test]
async fn chosen_stubs_and_summary_reach_the_provider_without_rewriting_the_journal() {
    let (session, _dir) = session("frame-layout");
    let mind = Mind::answering("frame-layout", "answer");
    let backend = backend(&mind);
    let tool = {
        let mut held = session.lock().await;
        held.commit(user("read a file")).expect("user");
        held.commit(Entry::Assistant {
            id: MessageId::new("a"),
            text: String::new(),
            thinking: "signed".into(),
            signatures: magi_proto::Signatures {
                thinking: Some("opaque".into()),
                ..Default::default()
            },
            stop_reason: Some(magi_proto::StopReason::ToolUse),
            error: None,
            usage: Default::default(),
        })
        .expect("assistant");
        held.commit(Entry::Tool {
            id: ToolCallId::new("read-1"),
            name: "read".into(),
            args: r#"{"path":"a.rs"}"#.into(),
            thought_signature: None,
            result: Some(ToolResult {
                output: "full original result".into(),
                is_error: false,
                images: vec![],
                shown: None,
            }),
        })
        .expect("tool")
    };
    let mut events = session.lock().await.subscribe();
    turn(&session, &backend).await;
    assert_eq!(frames(&mut events), [vec![]]);
    {
        let mut held = session.lock().await;
        held.commit(user("continue with the selected stub"))
            .expect("user");
        let mut layout = magi_host::laying::whole(&magi_host::laying::live(&held));
        for slot in &mut layout.slots {
            if slot.cursor() == Some(tool.0) {
                *slot = magi_proto::laying::Slot::Stub {
                    cursor: tool.0,
                    text: "selected stub".into(),
                };
            }
        }
        held.lay(layout);
    }
    turn(&session, &backend).await;
    assert_eq!(frames(&mut events), [vec![Invalidation::LayoutChanged]]);
    {
        let mut held = session.lock().await;
        let newest = held
            .commit(user("continue with the selected summary"))
            .expect("user");
        held.lay(
            serde_json::from_value(json!({"slots":[
                {"kind":"summary","text":"selected summary"}, {"kind":"item","cursor":newest.0}
            ]}))
            .expect("layout"),
        );
    }
    turn(&session, &backend).await;
    assert_eq!(
        frames(&mut events),
        [vec![
            Invalidation::SummaryActivated,
            Invalidation::LayoutReduced
        ]]
    );
    let asks = mind
        .asks()
        .iter()
        .map(|ask| serde_json::from_str::<magi_proto::ask::Ask>(ask).expect("ask"))
        .collect::<Vec<_>>();
    assert_eq!(asks.len(), 3);
    assert!(
        asks[1]
            .context
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|c| matches!(c,
        Content::ToolResult { id, content, .. } if id=="read-1" && content=="selected stub"))
    );
    assert_eq!(asks[1].context.messages[1], asks[0].context.messages[1]);
    assert_eq!(
        asks[2].context.messages[1].role,
        magi_model::Role::Assistant
    );
    assert!(
        asks[2].context.messages[1]
            .text()
            .contains("selected summary")
    );
    assert!(session.lock().await.entries().iter().any(|e| matches!(e,
        Entry::Tool { result:Some(result), .. } if result.output=="full original result")));
}
