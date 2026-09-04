//! Detach and reattach, over a real socket.
//!
//! The UI is expected to die and come back — that is the point of putting the harness in
//! another process. These drive the reduction the driver performs, against the transport the
//! driver uses, so the resume path is exercised end to end rather than asserted about.

use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, Entry, HarnessEvent, UiCommand};
use magi_testkit::{FakeHarness, Recording};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Short enough to stay under `SUN_LEN`, which a scratch directory path is not.
fn socket_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("magi-t-{}-{name}.sock", std::process::id()))
}

async fn serve(name: &str) -> PathBuf {
    let path = socket_path(name);
    let _ = std::fs::remove_file(&path);
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello.jsonl"
    ))
    .expect("sample recording");
    let recording = Recording::parse(&source).expect("parse");
    let listener = magi_ipc::bind(&path).await.expect("bind");
    let harness = FakeHarness::new(recording, Duration::ZERO);
    tokio::spawn(async move { harness.serve(listener).await });
    path
}

/// Attach, drain `take` events, and report what was seen.
async fn attach(path: &Path, from: Cursor, take: usize) -> (Vec<Entry>, Vec<HarnessEvent>) {
    let stream = magi_ipc::connect(path).await.expect("connect");
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: from,
        })
        .await
        .expect("attach");

    let snapshot: HarnessEvent = reader.read().await.expect("snapshot");
    let entries = match snapshot {
        HarnessEvent::SessionSnapshot { entries, .. } => entries,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut events = Vec::new();
    for _ in 0..take {
        match reader.read::<HarnessEvent>().await {
            Ok(event) => events.push(event),
            Err(_) => break,
        }
    }
    (entries, events)
}

#[tokio::test]
async fn a_cold_attach_replays_the_whole_session() {
    let path = serve("cold").await;
    let (entries, events) = attach(&path, Cursor::ZERO, 21).await;
    assert!(entries.is_empty(), "nothing precedes cursor zero");
    assert_eq!(events.len(), 21, "every recorded event streams");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn reattaching_mid_turn_resumes_without_replaying() {
    let path = serve("resume").await;

    // A UI attaches, reads part of the session, and dies.
    let (_, first) = attach(&path, Cursor::ZERO, 9).await;
    let last_seen = first.last().expect("events").cursor();
    assert_eq!(last_seen, Cursor(9));

    // It comes back with the cursor it had. History arrives as entries, not as events.
    let (entries, resumed) = attach(&path, last_seen, 12).await;

    assert!(
        !entries.is_empty(),
        "the snapshot carries what was already seen"
    );
    assert!(
        resumed.iter().all(|e| e.cursor() > last_seen),
        "nothing at or before the cursor is replayed"
    );
    assert_eq!(
        resumed.first().map(HarnessEvent::cursor),
        Some(Cursor(10)),
        "the stream continues at the next position"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_resumed_snapshot_carries_the_text_streamed_before_the_detach() {
    let path = serve("partial").await;

    // Cursor 6 lands inside the first assistant message, after one text delta.
    let (entries, _) = attach(&path, Cursor(6), 0).await;
    let streamed = entries
        .iter()
        .find_map(|e| match e {
            Entry::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        })
        .expect("an assistant entry");

    assert_eq!(
        streamed, "I'll look at the ",
        "an in-flight message resumes with exactly what it had"
    );
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn the_two_paths_agree_on_the_final_transcript() {
    let path = serve("agree").await;

    let (_, all) = attach(&path, Cursor::ZERO, 21).await;
    let cold = fold(Vec::new(), &all);

    let (entries, rest) = attach(&path, Cursor(9), 12).await;
    let warm = fold(entries, &rest);

    assert_eq!(
        cold, warm,
        "replaying from zero and resuming from a cursor reach the same transcript"
    );
    let _ = std::fs::remove_file(&path);
}

/// Apply events onto a starting transcript, the way the UI's `App` does.
fn fold(mut entries: Vec<Entry>, events: &[HarnessEvent]) -> Vec<Entry> {
    for event in events {
        match event.clone() {
            // Not part of a transcript: a question nobody is there to answer has no place in one,
            // and rows a tool held for a while are on the screen rather than in the record.
            HarnessEvent::Asked { .. } => {}
            HarnessEvent::PermissionAsked { .. } => {}
            HarnessEvent::Surfaced { .. }
            | HarnessEvent::Drew { .. }
            | HarnessEvent::Unsurfaced { .. } => {}
            HarnessEvent::UserMessage { id, text, .. } => entries.push(Entry::User {
                id,
                text,
                aside: String::new(),
            }),
            HarnessEvent::AssistantStarted { id, .. } => entries.push(Entry::Assistant {
                id,
                text: String::new(),
                thinking: String::new(),
                stop_reason: None,
                error: None,
                signatures: magi_proto::Signatures::default(),
                usage: magi_proto::Usage::default(),
            }),
            HarnessEvent::AssistantDelta { text, thinking, .. } => {
                if let Some(Entry::Assistant {
                    text: body,
                    thinking: reasoning,
                    ..
                }) = entries.last_mut()
                {
                    body.push_str(&text);
                    reasoning.push_str(&thinking);
                }
            }
            HarnessEvent::AssistantEnded {
                stop_reason, error, ..
            } => {
                if let Some(Entry::Assistant {
                    stop_reason: slot,
                    error: err,
                    ..
                }) = entries.last_mut()
                {
                    *slot = Some(stop_reason);
                    *err = error;
                }
            }
            HarnessEvent::Refused { .. } | HarnessEvent::ModelChanged { .. } => {}
            HarnessEvent::MessageArrived {
                who,
                kin,
                sort,
                text,
                ..
            } => entries.push(Entry::From {
                who,
                kin,
                sort,
                text,
            }),
            HarnessEvent::Branched { id, keeps, .. } => {
                entries.push(Entry::Branch { id, keeps });
            }
            HarnessEvent::Compacted {
                id,
                summary,
                replaces,
                ..
            } => entries.push(Entry::Compaction {
                id,
                summary,
                replaces,
            }),
            HarnessEvent::ToolCallStarted { id, name, args, .. } => entries.push(Entry::Tool {
                id,
                name,
                args,
                result: None,
                thought_signature: None,
            }),
            HarnessEvent::ToolCallEnded { result, .. } => {
                if let Some(Entry::Tool { result: slot, .. }) = entries.last_mut() {
                    *slot = Some(result);
                }
            }
            HarnessEvent::StatusChanged { .. }
            | HarnessEvent::SessionSnapshot { .. }
            | HarnessEvent::Error { .. } => {}
        }
    }
    entries
}
