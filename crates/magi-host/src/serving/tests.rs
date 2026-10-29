//! A screen that stops reading, against a session served over a real socket.

use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, Entry, HarnessEvent, UiCommand};
use std::time::Duration;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// A connection to `socket` that has attached with the session's key.
async fn attached(
    socket: &std::path::Path,
) -> (FrameReader<OwnedReadHalf>, FrameWriter<OwnedWriteHalf>) {
    let stream = magi_ipc::connect(socket).await.expect("connect");
    let (read_half, write_half) = stream.into_split();
    let mut writer = FrameWriter::new(write_half);
    let attach = UiCommand::Attach {
        session: None,
        from_cursor: Cursor::ZERO,
        draws: false,
        key: magi_ipc::read_key(socket),
    };
    writer.write(&attach).await.expect("attach");
    (FrameReader::new(read_half), writer)
}

/// Whether `event` shows the prompt `last`, as itself or inside a snapshot.
fn shows_last(event: &HarnessEvent) -> bool {
    let last = |entry: &Entry| matches!(entry, Entry::User { text, .. } if text == "last");
    match event {
        HarnessEvent::UserMessage { text, .. } => text == "last",
        HarnessEvent::SessionSnapshot { entries, .. } => entries.iter().any(last),
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tracked_native_prompt_preserves_admission_completion_and_refusal_identity() {
    let dir = Scratch::new("magi-host", "tracked");
    let socket = dir.join("s.sock");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let session = crate::open_session(1, "");
    let serving = tokio::spawn(crate::serve_writing(
        listener,
        session,
        None,
        crate::catalog::Catalog::empty(),
        None,
        Duration::from_secs(3),
    ));
    let (mut reader, mut writer) = attached(&socket).await;
    assert!(matches!(
        reader.read::<HarnessEvent>().await.expect("snapshot"),
        HarnessEvent::SessionSnapshot { .. }
    ));
    let tracked = |request, text: &str| UiCommand::SubmitTracked {
        request,
        text: text.into(),
        aside: String::new(),
        images: vec![],
    };
    writer.write(&tracked(8, "accepted")).await.expect("write");
    let mut started = None;
    let mut admitted = false;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match reader.read::<HarnessEvent>().await.expect("event") {
                HarnessEvent::TurnStarted {
                    owner,
                    request: Some(8),
                } => {
                    assert!(started.replace(owner).is_none());
                }
                HarnessEvent::UserMessage { text, .. } => {
                    assert!(started.is_some());
                    assert_eq!(text, "accepted");
                    admitted = true;
                }
                HarnessEvent::TurnFinished { owner } if Some(owner) == started => {
                    assert!(admitted);
                    break;
                }
                HarnessEvent::PromptRefused { .. } => panic!("unexpected refusal"),
                _ => {}
            }
        }
    })
    .await
    .expect("turn deadline");
    writer
        .write(&tracked(9, "/skill ../invalid"))
        .await
        .expect("invalid workflow");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let HarnessEvent::PromptRefused { request, .. } = reader.read().await.expect("event")
            {
                assert_eq!(request, 9);
                break;
            }
        }
    })
    .await
    .expect("refusal deadline");
    serving.abort();
    let _ = serving.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_screen_that_never_reads_is_let_go_and_holds_nobody_up() {
    let dir = Scratch::new("magi-host", "stalled");
    let socket = dir.join("s.sock");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let session = crate::open_session(1, "");
    let catalog = crate::catalog::Catalog::empty();
    let writing = Duration::from_millis(300);
    tokio::spawn(crate::serve_writing(
        listener, session, None, catalog, None, writing,
    ));

    let (mut stalled, _kept_open) = attached(&socket).await;
    let (mut reader, mut writer) = attached(&socket).await;
    let reading = tokio::spawn(async move {
        while let Ok(event) = reader.read::<HarnessEvent>().await {
            if shows_last(&event) {
                return true;
            }
        }
        false
    });
    for n in 0..200 {
        let text = format!("{n} {}", "x".repeat(16 * 1024));
        let prompt = UiCommand::SubmitPrompt {
            text,
            aside: String::new(),
            images: Vec::new(),
        };
        writer.write(&prompt).await.expect("submit");
    }
    let last = UiCommand::SubmitPrompt {
        text: "last".into(),
        aside: String::new(),
        images: Vec::new(),
    };
    writer.write(&last).await.expect("submit");

    let seen = tokio::time::timeout(Duration::from_secs(20), reading).await;
    assert!(
        seen.is_ok_and(|seen| seen.is_ok_and(|seen| seen)),
        "the screen that reads was held up by the one that does not"
    );
    // Past the deadline, then what it was sent before it was let go, and the end.
    tokio::time::sleep(writing * 4).await;
    let let_go = tokio::time::timeout(Duration::from_secs(20), async {
        while stalled.read::<HarnessEvent>().await.is_ok() {}
    })
    .await;
    assert!(
        let_go.is_ok(),
        "the screen that stopped reading was never let go"
    );
}

#[tokio::test]
async fn model_receipts_preserve_controller_and_native_busy_refusals() {
    for busy in [false, true] {
        let dir = Scratch::new(
            "magi-host",
            if busy { "model-busy" } else { "model-unknown" },
        );
        let socket = dir.join("s.sock");
        let listener = magi_ipc::bind(&socket).await.expect("bind");
        let mut session = crate::open_session(1, "");
        if busy {
            session
                .admit(crate::session::admission::Request::Grants(vec![]))
                .expect("busy");
        }
        let serving = tokio::spawn(crate::serve_writing(
            listener,
            session,
            None,
            crate::catalog::Catalog::empty(),
            None,
            Duration::from_secs(3),
        ));
        let (mut reader, mut writer) = attached(&socket).await;
        assert!(matches!(
            reader.read::<HarnessEvent>().await.expect("snapshot"),
            HarnessEvent::SessionSnapshot { .. }
        ));
        writer
            .write(&UiCommand::SelectModel {
                request: 71,
                name: "fake/one".into(),
            })
            .await
            .expect("select");
        let event = tokio::time::timeout(Duration::from_secs(3), reader.read::<HarnessEvent>())
            .await
            .expect("deadline")
            .expect("receipt");
        let HarnessEvent::ModelSelected {
            request,
            error: Some(error),
        } = event
        else {
            panic!("unexpected {event:?}");
        };
        assert_eq!(request, 71);
        assert_eq!(error.contains("busy"), busy);
        let stream = magi_ipc::connect(&socket).await.expect("watcher");
        let (read, write) = stream.into_split();
        let mut watcher = FrameReader::new(read);
        let mut commands = FrameWriter::new(write);
        commands
            .write(&UiCommand::AttachLatest {
                draws: false,
                key: None,
            })
            .await
            .expect("watch");
        assert!(matches!(
            watcher.read::<HarnessEvent>().await.expect("snapshot"),
            HarnessEvent::SessionSnapshot { .. }
        ));
        commands
            .write(&UiCommand::SelectModel {
                request: 72,
                name: "fake/one".into(),
            })
            .await
            .expect("forbidden");
        let event = tokio::time::timeout(Duration::from_secs(3), watcher.read::<HarnessEvent>())
            .await
            .expect("deadline")
            .expect("receipt");
        let HarnessEvent::ModelSelected {
            request,
            error: Some(error),
        } = event
        else {
            panic!("unexpected {event:?}");
        };
        assert_eq!(request, 72);
        assert!(error.contains("controller key"));
        serving.abort();
        let _ = serving.await;
    }
}

#[tokio::test]
async fn native_history_pages_require_controller_authority_and_preserve_boundaries() {
    let dir = Scratch::new("magi-host", "history-page");
    let socket = dir.join("s.sock");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let rows: Vec<_> = (1..140)
        .map(|n| {
            (
                Cursor(n * 2),
                Entry::User {
                    id: magi_proto::MessageId::new(format!("u{n}")),
                    text: format!("history {n}"),
                    aside: String::new(),
                    images: vec![],
                },
            )
        })
        .collect();
    let id = magi_proto::SessionId::new("historical-owner");
    let session = crate::session::Session::restored(id.clone(), rows.clone()).expect("restore");
    let serving = tokio::spawn(crate::serve_writing(
        listener,
        session,
        None,
        crate::catalog::Catalog::empty(),
        None,
        Duration::from_secs(3),
    ));
    let request = |after, through, key| UiCommand::ReadHistory {
        session: id.clone(),
        after,
        through,
        key,
    };
    let read = async |command| {
        let stream = magi_ipc::connect(&socket).await.expect("connect");
        let (reader, writer) = stream.into_split();
        FrameWriter::new(writer)
            .write(&command)
            .await
            .expect("request");
        tokio::time::timeout(
            Duration::from_secs(3),
            FrameReader::new(reader).read::<Result<magi_proto::HistoryPage, String>>(),
        )
        .await
        .expect("deadline")
        .expect("reply")
    };
    assert!(
        read(request(Cursor::ZERO, Cursor(278), None))
            .await
            .expect_err("watcher refused")
            .contains("controller key")
    );
    assert!(
        read(request(
            Cursor::ZERO,
            Cursor(276),
            magi_ipc::read_key(&socket)
        ))
        .await
        .is_err()
    );
    let first = read(request(
        Cursor::ZERO,
        Cursor(278),
        magi_ipc::read_key(&socket),
    ))
    .await
    .expect("first");
    assert_eq!(first.entries.len(), 128);
    assert_eq!(first.next, Some(Cursor(256)));
    let last = read(request(
        Cursor(256),
        Cursor(278),
        magi_ipc::read_key(&socket),
    ))
    .await
    .expect("last");
    assert_eq!(last.after, Cursor(256));
    assert!(last.next.is_none());
    assert_eq!(
        first
            .entries
            .into_iter()
            .chain(last.entries)
            .collect::<Vec<_>>(),
        rows
    );
    serving.abort();
    let _ = serving.await;
}
