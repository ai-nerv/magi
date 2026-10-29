#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use agent_client_protocol_schema::v1::{RequestId, SessionNotification};
use magi_proto::{
    Cursor, Entry, MessageId, SessionId, Signatures, StopReason, ToolCallId, ToolResult,
};

fn compact(summary: &str) -> HarnessEvent {
    HarnessEvent::Compacted {
        cursor: Cursor(9),
        id: MessageId::new("c9"),
        summary: summary.into(),
        replaces: 3,
    }
}

fn schema(frame: &Value) {
    let parsed: SessionNotification =
        serde_json::from_value(frame["params"].clone()).expect("official schema");
    assert_eq!(
        serde_json::to_value(parsed).expect("encoded schema"),
        frame["params"]
    );
}

#[test]
fn compaction_negotiation_requires_a_valid_initialized_capability() {
    for (caps, expected) in [
        (json!({}), false),
        (json!({"session":null}), false),
        (json!({"session":{}}), false),
        (json!({"session":{"compaction":null}}), false),
        (json!({"session":{"compaction":{}}}), true),
    ] {
        let mut state = crate::acp::initialize::State::default();
        assert!(!state.display().compaction);
        let response = state.request(
            RequestId::Number(1),
            "initialize",
            json!({
                "protocolVersion":1, "clientCapabilities":caps
            }),
        );
        assert!(response.get("result").is_some(), "{response}");
        assert_eq!(state.display().compaction, expected);
        let retry = state.request(
            RequestId::Number(2),
            "initialize",
            json!({
                "protocolVersion":1, "clientCapabilities":{"session":{"compaction":{}}}
            }),
        );
        assert!(retry.get("error").is_some());
        assert_eq!(state.display().compaction, expected);
    }
}

#[test]
fn native_compaction_is_a_stable_terminal_replacement_or_ordinary_message() {
    let policy = Policy { compaction: true };
    let complete = policy
        .boundary("external", &compact("retained summary"))
        .expect("update");
    schema(&complete);
    assert_eq!(
        complete["params"]["update"]["compactionId"],
        "magi-compaction-c9"
    );
    assert_eq!(complete["params"]["update"]["status"], "completed");
    assert_eq!(
        complete["params"]["update"]["summary"][0]["text"],
        "retained summary"
    );
    assert_eq!(
        complete,
        policy
            .boundary("external", &compact("retained summary"))
            .expect("replay")
    );
    let empty = policy
        .boundary("external", &compact(""))
        .expect("empty summary");
    schema(&empty);
    assert_eq!(empty["params"]["update"]["summary"], json!([]));
    for text in ["retained summary", ""] {
        let fallback = Policy::default()
            .boundary("external", &compact(text))
            .expect("fallback");
        schema(&fallback);
        assert_eq!(
            fallback["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );
        assert!(fallback["params"]["update"].get("compactionId").is_none());
        assert!(
            fallback["params"]["update"]["content"]["text"]
                .as_str()
                .expect("text")
                .contains(text)
        );
    }
}

#[test]
fn native_history_display_keeps_content_but_not_private_provider_context() {
    let image = magi_model::Image {
        media_type: "image/png".into(),
        data: "aW1hZ2U=".into(),
    };
    let rows = vec![
        Entry::User {
            id: MessageId::new("u1"),
            text: "visible user".into(),
            aside: "private-aside".into(),
            images: vec![image.clone()],
        },
        Entry::Assistant {
            id: MessageId::new("a2"),
            text: "visible answer".into(),
            thinking: "visible thinking".into(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            signatures: Signatures {
                text: Some("private-text-signature".into()),
                thinking: Some("private-thinking-signature".into()),
            },
            usage: Default::default(),
        },
        Entry::Tool {
            id: ToolCallId::new("t3"),
            name: "historical-tool".into(),
            args: "{\"literal\":true}".into(),
            result: Some(ToolResult {
                images: vec![image],
                output: "historical output".into(),
                is_error: false,
                shown: None,
            }),
            thought_signature: Some("private-tool-signature".into()),
        },
        Entry::Masked {
            id: MessageId::new("m4"),
            at: 2,
            shown: "private-mask".into(),
        },
        Entry::From {
            who: "child".into(),
            kin: "agent".into(),
            sort: "note".into(),
            text: "visible arrival".into(),
        },
        Entry::Branch {
            id: MessageId::new("b6"),
            keeps: 1,
        },
        Entry::Compaction {
            id: MessageId::new("c7"),
            summary: "visible summary".into(),
            replaces: 2,
        },
    ];
    let session = magi_host::session::Session::recorded(SessionId::new("native"), rows);
    let before = session.entries().to_vec();
    for compaction in [false, true] {
        let policy = Policy { compaction };
        let mut turn = crate::acp::turns::Turn::background();
        let mut frames = Vec::new();
        for event in session.replay(Cursor::ZERO) {
            if let Some(frame) = policy.boundary("external", &event) {
                frames.push(frame);
            } else {
                let update = turn.event("external", event).expect("display projection");
                assert!(!update.interrupt);
                assert!(
                    update.complete.is_none(),
                    "history cannot create a prompt reply"
                );
                frames.extend(update.frames);
            }
        }
        for frame in &frames {
            schema(frame);
        }
        let encoded = serde_json::to_string(&frames).expect("frames");
        for visible in [
            "visible user",
            "visible answer",
            "visible thinking",
            "historical output",
            "visible arrival",
            "visible summary",
            "Context rewound",
            "aW1hZ2U=",
        ] {
            assert!(encoded.contains(visible), "missing {visible}");
        }
        assert!(!encoded.contains("private-"));
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame["params"]["update"]["sessionUpdate"] == "compaction_update")
                .count(),
            usize::from(compaction)
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame["params"]["update"]["sessionUpdate"] == "tool_call")
                .count(),
            1
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame["params"]["update"]["sessionUpdate"] == "tool_call_update")
                .count(),
            1
        );
    }
    assert_eq!(session.entries(), before);
}
