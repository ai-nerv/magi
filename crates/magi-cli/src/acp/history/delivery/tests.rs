#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use agent_client_protocol_schema::v1::SessionNotification;
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Entry, HistoryPage, MessageId, SessionId, UiCommand};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

fn channel() -> (Replay, mpsc::Receiver<Delivery>) {
    let (output, receive) = mpsc::channel(1);
    (
        Replay {
            generation: 7,
            policy: display::Policy { compaction: true },
            output,
        },
        receive,
    )
}

fn identity() -> events::Identity {
    events::Identity {
        external: crate::acp::history::external("owner@child").expect("external identity"),
        native: SessionId::new("owner"),
        generation: 7,
    }
}

fn user(index: u64) -> Entry {
    Entry::User {
        id: MessageId::new(format!("u{index}")),
        text: format!("visible-{index}"),
        aside: "private-aside".into(),
        images: vec![],
    }
}

fn page(after: u64, through: u64, entries: Vec<(Cursor, Entry)>, next: Option<u64>) -> HistoryPage {
    HistoryPage {
        session: identity().native,
        transcript: Some("owner@child".into()),
        through: Cursor(through),
        after: Cursor(after),
        next: next.map(Cursor),
        entries,
    }
}

fn fixture(pages: Vec<HistoryPage>) -> (Scratch, std::path::PathBuf, tokio::task::JoinHandle<()>) {
    let root = Scratch::new("acp-replay", "delivery");
    let socket = root.join("host");
    let listener = tokio::net::UnixListener::bind(&socket).expect("listener");
    let server = tokio::spawn(async move {
        for page in pages {
            let (stream, _) = listener.accept().await.expect("page connection");
            let (read, write) = stream.into_split();
            let command = FrameReader::new(read)
                .read::<UiCommand>()
                .await
                .expect("history request");
            assert!(
                matches!(command, UiCommand::ReadHistory {session, through, after, key:Some(key)}
                if session == identity().native && through == page.through && after == page.after && key == "controller")
            );
            let answer: std::result::Result<HistoryPage, String> = Ok(page);
            FrameWriter::new(write)
                .write(&answer)
                .await
                .expect("history answer");
        }
    });
    (root, socket, server)
}

#[tokio::test]
async fn replay_waits_for_the_complete_output_write_not_queue_admission() {
    let (replay, mut receive) = channel();
    let expected = display::chunk("session", "user_message_chunk", "visible");
    let frame = expected.clone();
    let sending = tokio::spawn(async move { replay.send(frame).await });
    let delivery = receive.recv().await.expect("queued delivery");
    tokio::task::yield_now().await;
    assert!(!sending.is_finished(), "enqueue is not delivery");
    let (mut output, input) = tokio::io::duplex(1);
    let writing = tokio::spawn(async move { delivery.write(&mut output).await });
    tokio::task::yield_now().await;
    assert!(!writing.is_finished());
    assert!(!sending.is_finished(), "partial output is not delivery");
    let mut input = BufReader::new(input);
    let mut line = String::new();
    input.read_line(&mut line).await.expect("complete frame");
    assert!(line.ends_with('\n'));
    assert_eq!(
        serde_json::from_str::<Value>(&line).expect("JSON"),
        expected
    );
    writing.await.expect("writer").expect("delivery");
    sending.await.expect("producer").expect("receipt");
}

#[tokio::test(start_paused = true)]
async fn replay_deadline_covers_both_queue_and_receipt_waits() {
    let (replay, mut receive) = channel();
    let frame = json!({"test":"frame"});
    let (accepted, _receipt) = oneshot::channel();
    replay
        .output
        .send(Delivery {
            generation: 7,
            frame: frame.clone(),
            deadline: Instant::now() + transport::DEADLINE,
            accepted,
        })
        .await
        .expect("fill queue");
    let sending = tokio::spawn(async move { replay.send(frame).await });
    tokio::task::yield_now().await;
    tokio::time::advance(transport::DEADLINE / 2).await;
    drop(receive.recv().await.expect("queued predecessor"));
    let delivery = receive.recv().await.expect("waiting delivery");
    assert!(!sending.is_finished());
    tokio::time::advance(transport::DEADLINE / 2).await;
    assert!(sending.await.expect("producer").is_err());
    let mut output = Vec::new();
    delivery
        .write(&mut output)
        .await
        .expect("stale delivery ignored");
    assert!(output.is_empty());
}

#[tokio::test(start_paused = true)]
async fn expired_or_abandoned_replay_frames_are_never_written() {
    for expired in [true, false] {
        let (accepted, receipt) = oneshot::channel();
        let delivery = Delivery {
            generation: 7,
            frame: json!({"private":"stale"}),
            deadline: Instant::now() + transport::DEADLINE,
            accepted,
        };
        if expired {
            tokio::time::advance(transport::DEADLINE).await;
        } else {
            drop(receipt);
        }
        let mut output = Vec::new();
        delivery.write(&mut output).await.expect("ignored");
        assert!(output.is_empty());
    }
}

#[tokio::test]
async fn closed_output_dropped_delivery_and_stale_generation_fail_the_producer() {
    for failure in ["closed-queue", "dropped", "stale", "write"] {
        let (replay, mut receive) = channel();
        let sending = tokio::spawn(async move { replay.send(json!({"test":"frame"})).await });
        if failure == "closed-queue" {
            drop(receive);
        } else {
            let delivery = receive.recv().await.expect("delivery");
            match failure {
                "dropped" => drop(delivery),
                "stale" => {
                    let mut sessions = crate::acp::sessions::Sessions::default();
                    let mut output = Vec::new();
                    sessions
                        .replay(delivery, &mut output)
                        .await
                        .expect("stale generation");
                    assert!(output.is_empty());
                }
                _ => {
                    let (mut output, input) = tokio::io::duplex(16);
                    drop(input);
                    assert!(delivery.write(&mut output).await.is_err());
                }
            }
        }
        assert!(sending.await.expect("producer").is_err(), "{failure}");
    }
}

#[tokio::test(start_paused = true)]
async fn abandoned_inflight_replay_fails_output_before_the_write_deadline() {
    let (accepted, receipt) = oneshot::channel();
    let delivery = Delivery {
        generation: 7,
        frame: json!({"test":"partial frame"}),
        deadline: Instant::now() + transport::DEADLINE,
        accepted,
    };
    let (mut output, mut input) = tokio::io::duplex(1);
    let writing = tokio::spawn(async move { delivery.write(&mut output).await });
    input.read_exact(&mut [0; 1]).await.expect("partial output");
    drop(receipt);
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), writing)
        .await
        .expect("abandoned output must not wait for its write deadline")
        .expect("writer task");
    assert!(
        result.is_err(),
        "a partial protocol frame cannot be followed by more output"
    );
}

#[tokio::test(start_paused = true)]
async fn stalled_output_write_expires_without_an_acknowledgement() {
    let (accepted, receipt) = oneshot::channel();
    let delivery = Delivery {
        generation: 7,
        frame: json!({"test":"too large for output"}),
        deadline: Instant::now() + transport::DEADLINE,
        accepted,
    };
    let (mut output, _input) = tokio::io::duplex(1);
    assert!(delivery.write(&mut output).await.is_err());
    assert!(receipt.await.is_err());
}

#[tokio::test]
async fn full_replay_preserves_sparse_multi_page_order_and_waits_for_the_last_receipt() {
    let pages = vec![
        page(
            0,
            390,
            (1..=128).map(|i| (Cursor(i * 3), user(i))).collect(),
            Some(384),
        ),
        page(
            384,
            390,
            (129..=130).map(|i| (Cursor(i * 3), user(i))).collect(),
            None,
        ),
    ];
    let (_root, socket, server) = fixture(pages);
    let (replay, mut receive) = channel();
    let sending = tokio::spawn(async move {
        replay
            .run(&socket, "controller", &identity(), Cursor(390))
            .await
    });
    for index in 1..=130 {
        let delivery = receive.recv().await.expect("display entry");
        assert_eq!(delivery.generation, 7);
        assert_eq!(
            delivery.frame,
            display::chunk(
                &identity().external,
                "user_message_chunk",
                &format!("visible-{index}")
            )
        );
        tokio::task::yield_now().await;
        assert!(
            receive.try_recv().is_err(),
            "one outstanding frame per replay"
        );
        assert!(!sending.is_finished(), "including the final output receipt");
        let mut output = Vec::new();
        delivery.write(&mut output).await.expect("written entry");
        assert!(output.ends_with(b"\n"));
        assert!(
            !String::from_utf8(output)
                .expect("UTF8")
                .contains("private-aside")
        );
    }
    sending
        .await
        .expect("replay")
        .expect("terminal page and receipts");
    server.await.expect("server");
    assert!(receive.recv().await.is_none());
}

#[tokio::test]
async fn replay_rejects_identity_changes_without_output() {
    let mut wrong = page(0, 3, vec![(Cursor(3), user(1))], None);
    wrong.transcript = Some("other".into());
    let (_root, socket, server) = fixture(vec![wrong]);
    let (replay, mut receive) = channel();
    let mut wrong_generation = identity();
    wrong_generation.generation += 1;
    assert!(
        replay
            .run(
                Path::new("/not-opened"),
                "controller",
                &wrong_generation,
                Cursor(3)
            )
            .await
            .is_err()
    );
    assert!(
        replay
            .run(&socket, "controller", &identity(), Cursor(3))
            .await
            .is_err()
    );
    server.await.expect("server");
    assert!(receive.try_recv().is_err());
}

#[tokio::test]
async fn empty_replay_completes_without_emitting_a_prompt_response() {
    let (_root, socket, server) = fixture(vec![page(0, 0, vec![], None)]);
    let (replay, mut receive) = channel();
    replay
        .run(&socket, "controller", &identity(), Cursor::ZERO)
        .await
        .expect("empty history");
    server.await.expect("server");
    assert!(receive.try_recv().is_err());
}

#[tokio::test]
async fn replay_projects_native_content_and_boundaries_without_private_state_or_actions() {
    use magi_proto::{Signatures, StopReason, ToolCallId, ToolResult};
    let image = magi_model::Image {
        media_type: "image/png".into(),
        data: "aW1hZ2U=".into(),
    };
    let entries = vec![
        Entry::User {
            id: MessageId::new("u"),
            text: "visible user".into(),
            aside: "private-aside".into(),
            images: vec![image.clone()],
        },
        Entry::Assistant {
            id: MessageId::new("a"),
            text: "visible answer".into(),
            thinking: "visible thought".into(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            usage: Default::default(),
            signatures: Signatures {
                text: Some("private-signature".into()),
                thinking: None,
            },
        },
        Entry::Tool {
            id: ToolCallId::new("t"),
            name: "historical-tool".into(),
            args: "literal".into(),
            result: Some(ToolResult {
                output: "visible result".into(),
                is_error: false,
                shown: None,
                images: vec![image],
            }),
            thought_signature: Some("private-tool-signature".into()),
        },
        Entry::Masked {
            id: MessageId::new("m"),
            at: 2,
            shown: "private-mask".into(),
        },
        Entry::Notice {
            text: "private-notice".into(),
        },
        Entry::Branch {
            id: MessageId::new("b"),
            keeps: 1,
        },
        Entry::Compaction {
            id: MessageId::new("c"),
            summary: "visible summary".into(),
            replaces: 2,
        },
    ];
    let rows = entries
        .into_iter()
        .zip(1..)
        .map(|(entry, cursor)| (Cursor(cursor), entry))
        .collect();
    let (_root, socket, server) = fixture(vec![page(0, 7, rows, None)]);
    let (replay, mut receive) = channel();
    let sending = tokio::spawn(async move {
        replay
            .run(&socket, "controller", &identity(), Cursor(7))
            .await
    });
    let mut frames = Vec::new();
    while let Some(delivery) = receive.recv().await {
        let frame = delivery.frame.clone();
        assert_eq!(frame["method"], "session/update");
        assert!(frame.get("id").is_none());
        let parsed: SessionNotification =
            serde_json::from_value(frame["params"].clone()).expect("official schema");
        assert_eq!(
            serde_json::to_value(parsed).expect("schema encoding"),
            frame["params"]
        );
        delivery
            .write(&mut Vec::new())
            .await
            .expect("display write");
        frames.push(frame);
    }
    sending.await.expect("producer").expect("read-only replay");
    server.await.expect("server");
    let kinds: Vec<_> = frames
        .iter()
        .map(|frame| {
            frame["params"]["update"]["sessionUpdate"]
                .as_str()
                .expect("update kind")
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "user_message_chunk",
            "user_message_chunk",
            "agent_message_chunk",
            "agent_thought_chunk",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk",
            "compaction_update"
        ]
    );
    let text = serde_json::to_string(&frames).expect("frames");
    assert!(!text.contains("private-"));
    for expected in [
        "visible user",
        "visible answer",
        "visible thought",
        "visible result",
        "visible summary",
        "aW1hZ2U=",
        "Context rewound",
    ] {
        assert!(text.contains(expected), "{expected}");
    }
}
