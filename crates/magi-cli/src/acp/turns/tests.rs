#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use magi_proto::{AgentStatus, Cursor};
use tokio::io::AsyncWriteExt;

async fn turn() -> Turn {
    let (mut source, reader) = tokio::io::duplex(128);
    let mut intake = crate::acp::intake::Intake::new(reader);
    source
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"session/prompt\"}\n")
        .await
        .expect("request");
    let item = intake.incoming.recv().await.expect("leased request");
    intake.finish().await;
    Turn::new(RequestId::Number(7), item)
}

fn idle() -> HarnessEvent {
    HarnessEvent::StatusChanged {
        cursor: Cursor(10),
        status: AgentStatus::Idle,
    }
}

fn admitted() -> HarnessEvent {
    HarnessEvent::UserMessage {
        cursor: Cursor(1),
        id: MessageId::new("u1"),
        text: "prompt".into(),
        images: vec![],
    }
}

fn ended(reason: StopReason) -> HarnessEvent {
    HarnessEvent::AssistantEnded {
        cursor: Cursor(5),
        id: MessageId::new("a2"),
        stop_reason: reason,
        error: None,
        usage: Default::default(),
    }
}

#[tokio::test]
async fn native_turn_finishes_only_after_admission_and_native_settlement() {
    let mut turn = turn().await;
    assert!(
        turn.event("s", idle())
            .expect("old idle")
            .complete
            .is_none()
    );
    let update = turn.event("s", admitted()).expect("admission");
    let _: agent_client_protocol_schema::v1::SessionNotification =
        serde_json::from_value(update.frames[0]["params"].clone()).expect("official update");
    assert!(turn.deadline.is_none());
    assert!(
        turn.event("s", ended(StopReason::ToolUse))
            .expect("tool round")
            .complete
            .is_none()
    );
    assert!(
        turn.event("s", ended(StopReason::EndTurn))
            .expect("answer")
            .complete
            .is_none()
    );
    assert!(
        turn.event("s", idle())
            .expect("status only")
            .complete
            .is_none()
    );
    let response = turn.finish("s").complete.expect("completion");
    assert_eq!(response["id"], 7);
    assert_eq!(response["result"]["stopReason"], "end_turn");
}

#[tokio::test]
async fn native_turn_retry_resets_error_but_final_flush_error_remains_failure() {
    let mut turn = turn().await;
    turn.event("s", admitted()).expect("admission");
    turn.event("s", ended(StopReason::Error))
        .expect("retryable error");
    turn.event(
        "s",
        HarnessEvent::AssistantStarted {
            cursor: Cursor(6),
            id: MessageId::new("a2"),
        },
    )
    .expect("retry");
    let update = turn
        .event(
            "s",
            HarnessEvent::AssistantDelta {
                cursor: Cursor(7),
                id: MessageId::new("a2"),
                text: "answer".into(),
                thinking: "thought".into(),
            },
        )
        .expect("delta");
    assert_eq!(update.frames.len(), 2);
    for frame in update.frames {
        let _: agent_client_protocol_schema::v1::SessionNotification =
            serde_json::from_value(frame["params"].clone()).expect("official delta");
    }
    turn.event("s", ended(StopReason::EndTurn)).expect("answer");
    turn.event("s", ended(StopReason::Error))
        .expect("flush failure");
    assert!(
        turn.finish("s")
            .complete
            .expect("completion")
            .get("error")
            .is_some()
    );
}

#[tokio::test]
async fn native_turn_cancel_before_admission_waits_for_native_refusal() {
    let mut turn = turn().await;
    assert!(turn.cancel());
    assert!(!turn.cancel());
    assert!(
        turn.event("s", idle())
            .expect("old idle")
            .complete
            .is_none()
    );
    let reply = turn
        .event(
            "s",
            HarnessEvent::PromptRefused {
                request: 1,
                message: "cancelled preparation".into(),
            },
        )
        .expect("refused")
        .complete
        .expect("completion");
    assert_eq!(reply["result"]["stopReason"], "cancelled");
}

#[tokio::test]
async fn native_turn_rejects_uncorrelated_deltas_and_duplicate_admissions() {
    let mut turn = turn().await;
    assert!(
        turn.event(
            "s",
            HarnessEvent::AssistantDelta {
                cursor: Cursor(1),
                id: MessageId::new("unknown"),
                text: "x".into(),
                thinking: String::new()
            }
        )
        .is_err()
    );
    turn.event("s", admitted()).expect("admitted");
    assert!(turn.event("s", admitted()).is_err());
}

async fn stream() -> (Stream, u64) {
    let mut stream = Stream::default();
    let mut turn = turn().await;
    let key = stream
        .submit(RequestId::Number(7), turn._lease.take().expect("lease"))
        .expect("submit");
    (stream, key)
}

fn started(owner: u64, request: Option<u64>) -> HarnessEvent {
    HarnessEvent::TurnStarted { owner, request }
}

#[tokio::test]
async fn background_turn_cannot_admit_or_finish_pending_client_prompt() {
    let (mut stream, key) = stream().await;
    stream.event("s", started(1, None)).expect("background");
    let report = stream
        .event(
            "s",
            HarnessEvent::MessageArrived {
                cursor: Cursor(1),
                who: "child".into(),
                kin: "subagent".into(),
                sort: "report".into(),
                text: "report revision".into(),
            },
        )
        .expect("report");
    assert_eq!(
        report.frames[0]["params"]["update"]["content"]["text"],
        "[subagent:child report]\nreport revision"
    );
    stream
        .event("s", ended(StopReason::EndTurn))
        .expect("background answer");
    assert!(
        stream
            .event("s", HarnessEvent::TurnFinished { owner: 1 })
            .expect("background finish")
            .complete
            .is_none()
    );
    stream.release();
    assert!(stream.busy(), "foreground admission still pending");
    stream
        .event("s", started(2, Some(key)))
        .expect("foreground");
    stream.event("s", admitted()).expect("prompt");
    stream
        .event("s", ended(StopReason::EndTurn))
        .expect("answer");
    let reply = stream
        .event("s", HarnessEvent::TurnFinished { owner: 2 })
        .expect("finish")
        .complete
        .expect("reply");
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["result"]["stopReason"], "end_turn");
    stream.release();
    assert!(!stream.busy());
    assert!(
        stream.event("s", started(2, None)).is_err(),
        "duplicate owner rejected"
    );
}

#[tokio::test]
async fn racing_foreground_refusal_does_not_finish_the_background_turn() {
    let (mut stream, key) = stream().await;
    stream.event("s", started(4, None)).expect("background");
    let reply = stream
        .event(
            "s",
            HarnessEvent::PromptRefused {
                request: key,
                message: "busy".into(),
            },
        )
        .expect("refusal")
        .complete
        .expect("reply");
    assert_eq!(reply["id"], 7);
    assert!(reply.get("error").is_some());
    stream.release();
    assert!(stream.busy());
    stream
        .event("s", ended(StopReason::EndTurn))
        .expect("background answer");
    assert!(
        stream
            .event("s", HarnessEvent::TurnFinished { owner: 4 })
            .expect("background finish")
            .complete
            .is_none()
    );
    stream.release();
    assert!(!stream.busy());
}

#[tokio::test]
async fn cancellation_and_wrong_id_receipts_cannot_cross_prompt_ownership() {
    let (mut stream, key) = stream().await;
    stream.event("s", started(1, None)).expect("background");
    assert!(stream.cancel());
    assert!(stream.cancelled());
    assert!(stream.deadline().is_some());
    assert!(
        stream
            .event(
                "s",
                HarnessEvent::PromptRefused {
                    request: key + 1,
                    message: "wrong".into()
                }
            )
            .is_err()
    );
    assert!(
        stream
            .event("s", HarnessEvent::TurnFinished { owner: 2 })
            .is_err()
    );
    assert!(
        stream
            .event("s", HarnessEvent::TurnFinished { owner: 1 })
            .expect("background finish")
            .complete
            .is_none()
    );
    stream.release();
    let reply = stream
        .event(
            "s",
            HarnessEvent::PromptRefused {
                request: key,
                message: "cancelled".into(),
            },
        )
        .expect("cancel")
        .complete
        .expect("reply");
    assert_eq!(reply["result"]["stopReason"], "cancelled");
    stream.release();
    let mut turn = turn().await;
    let next = stream
        .submit(RequestId::Number(8), turn._lease.take().expect("lease"))
        .expect("next");
    assert_ne!(next, key);
    assert!(!stream.cancelled());
    assert!(stream.event("s", started(2, Some(key))).is_err());
    stream
        .event("s", started(2, Some(next)))
        .expect("correct admission");
}

#[test]
fn background_failures_are_updates_not_invented_client_responses() {
    let mut turn = Turn::background();
    turn.event("s", ended(StopReason::Error)).expect("failure");
    let update = turn.finish("s");
    assert!(update.complete.is_none());
    assert_eq!(
        update.frames[0]["params"]["update"]["content"]["text"],
        "Automatic native turn failed"
    );
    let _: agent_client_protocol_schema::v1::SessionNotification =
        serde_json::from_value(update.frames[0]["params"].clone()).expect("official update");
    turn.unsupported = true;
    assert!(
        turn.finish("s").frames[0]["params"]["update"]["content"]["text"]
            .as_str()
            .expect("notice")
            .contains("unsupported")
    );
    turn.cancel();
    assert!(
        turn.finish("s").frames[0]["params"]["update"]["content"]["text"]
            .as_str()
            .expect("notice")
            .contains("cancelled")
    );
}
