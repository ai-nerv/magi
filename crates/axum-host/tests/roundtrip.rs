//! The daemon, over a real socket.
//!
//! M1's claim is that `axum host` can stand in for the replay host without the UI changing.
//! These drive the same protocol the UI drives, so the claim is tested rather than asserted.

use axum_host::{open_session, serve};
use axum_ipc::{FrameReader, FrameWriter};
use axum_proto::{Cursor, Entry, HarnessEvent, UiCommand};
use std::path::{Path, PathBuf};

fn temp(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("axum-host-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    // Short enough to stay under SUN_LEN, which a scratch directory path is not.
    let socket = std::env::temp_dir().join(format!("axum-h-{}-{name}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    (dir, socket)
}

async fn start(name: &str) -> (PathBuf, PathBuf) {
    let (dir, socket) = temp(name);
    let session = open_session(&dir, "/tmp", 1).expect("session");
    let listener = axum_ipc::bind(&socket).await.expect("bind");
    tokio::spawn(async move { serve(listener, session, None).await });
    (dir, socket)
}

struct Client {
    reader: FrameReader<tokio::net::unix::OwnedReadHalf>,
    writer: FrameWriter<tokio::net::unix::OwnedWriteHalf>,
}

impl Client {
    async fn attach(socket: &Path, from: Cursor) -> (Self, Vec<Entry>) {
        let stream = axum_ipc::connect(socket).await.expect("connect");
        let (read_half, write_half) = stream.into_split();
        let mut client = Self {
            reader: FrameReader::new(read_half),
            writer: FrameWriter::new(write_half),
        };
        client
            .writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: from,
            })
            .await
            .expect("attach");
        match client
            .reader
            .read::<HarnessEvent>()
            .await
            .expect("snapshot")
        {
            HarnessEvent::SessionSnapshot { entries, .. } => (client, entries),
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    async fn submit(&mut self, text: &str) {
        self.writer
            .write(&UiCommand::SubmitPrompt { text: text.into() })
            .await
            .expect("submit");
    }

    async fn next(&mut self) -> HarnessEvent {
        self.reader.read().await.expect("an event")
    }
}

#[tokio::test]
async fn a_submitted_prompt_comes_back_as_events() {
    let (dir, socket) = start("submit").await;
    let (mut client, entries) = Client::attach(&socket, Cursor::ZERO).await;
    assert!(entries.is_empty(), "a fresh session has no history");

    client.submit("hello").await;

    let first = client.next().await;
    match first {
        HarnessEvent::UserMessage { text, .. } => assert_eq!(text, "hello"),
        other => panic!("expected the user message first, got {other:?}"),
    }
    assert!(matches!(
        client.next().await,
        HarnessEvent::AssistantStarted { .. }
    ));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn a_prompt_survives_the_ui_and_is_replayed_on_reattach() {
    let (dir, socket) = start("reattach").await;
    {
        let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
        client.submit("remember me").await;
        // Drain until the turn settles, then drop the connection as a dying UI would.
        for _ in 0..4 {
            client.next().await;
        }
    }

    let (_client, entries) = Client::attach(&socket, Cursor(2)).await;
    assert_eq!(entries.len(), 2, "the snapshot carries what was committed");
    match &entries[0] {
        Entry::User { text, .. } => assert_eq!(text, "remember me"),
        other => panic!("expected a user entry, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn a_cold_reattach_replays_the_whole_session() {
    let (dir, socket) = start("cold").await;
    {
        let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
        client.submit("one").await;
        for _ in 0..4 {
            client.next().await;
        }
    }

    let (mut client, entries) = Client::attach(&socket, Cursor::ZERO).await;
    assert!(entries.is_empty(), "a cold attach carries no history");
    assert!(
        matches!(client.next().await, HarnessEvent::UserMessage { .. }),
        "it arrives as replayed events instead"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn two_uis_both_see_a_prompt_either_one_submits() {
    let (dir, socket) = start("two").await;
    let (mut a, _) = Client::attach(&socket, Cursor::ZERO).await;
    let (mut b, _) = Client::attach(&socket, Cursor::ZERO).await;

    a.submit("shared").await;

    for client in [&mut a, &mut b] {
        match client.next().await {
            HarnessEvent::UserMessage { text, .. } => assert_eq!(text, "shared"),
            other => panic!("both attached UIs see it, got {other:?}"),
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}

#[tokio::test]
async fn the_journal_outlives_the_daemon() {
    let (dir, socket) = start("durable").await;
    {
        let (mut client, _) = Client::attach(&socket, Cursor::ZERO).await;
        client.submit("persisted").await;
        for _ in 0..4 {
            client.next().await;
        }
    }

    let journal = std::fs::read_dir(&dir)
        .expect("read dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .expect("a journal");
    let source = std::fs::read_to_string(&journal).expect("read");
    assert!(source.contains("persisted"), "the prompt reached the disk");
    assert!(source.lines().count() >= 3, "meta, prompt, reply");

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&socket);
}
