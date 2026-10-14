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
        };
        writer.write(&prompt).await.expect("submit");
    }
    let last = UiCommand::SubmitPrompt {
        text: "last".into(),
        aside: String::new(),
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
