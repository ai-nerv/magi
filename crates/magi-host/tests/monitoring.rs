//! Read-only host inspection and atomic tail attachment over real sockets.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{open_session, serve};
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{Entry, HarnessEvent, UiCommand, monitoring::AgentDetails};
use std::time::Duration;

#[tokio::test]
async fn inspection_does_not_subscribe_or_disclose_history() {
    let dir = Scratch::new("magi-host", "inspection");
    let socket = dir.join("s.sock");
    let mut session = open_session(1, "");
    session
        .commit(Entry::Notice {
            text: "private transcript".into(),
        })
        .expect("commit");
    let events = session.publisher();
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let server = tokio::spawn(serve(listener, session, None));
    let stream = magi_ipc::connect(&socket).await.expect("connect");
    let (read, write) = stream.into_split();
    FrameWriter::new(write)
        .write(&UiCommand::Inspect)
        .await
        .expect("inspect");
    let mut reader = FrameReader::new(read);
    let details = reader.read::<AgentDetails>().await.expect("details");
    assert!(details.model.is_none());
    assert!(details.context.is_none());
    assert_eq!((details.approvals, details.questions), (0, 0));
    assert_eq!(events.receiver_count(), 0);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), reader.read::<AgentDetails>())
            .await
            .expect("closed")
            .is_err()
    );
    server.abort();
}

#[tokio::test]
async fn attach_latest_starts_with_the_tail_and_only_then_streams_new_events() {
    let dir = Scratch::new("magi-host", "tail-attach");
    let socket = dir.join("s.sock");
    let mut session = open_session(2, "");
    for index in 0..200 {
        session
            .commit(Entry::Notice {
                text: format!("history-{index}"),
            })
            .expect("commit");
    }
    let before = session.cursor();
    let events = session.publisher();
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let server = tokio::spawn(serve(listener, session, None));
    let stream = magi_ipc::connect(&socket).await.expect("connect");
    let (read, write) = stream.into_split();
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::AttachLatest {
            draws: false,
            key: magi_ipc::read_key(&socket),
        })
        .await
        .expect("attach");
    let mut reader = FrameReader::new(read);
    let HarnessEvent::SessionSnapshot {
        cursor, entries, ..
    } = reader.read().await.expect("snapshot")
    else {
        panic!("first event must be a snapshot");
    };
    assert_eq!(cursor, before);
    assert_eq!(
        entries.last(),
        Some(&Entry::Notice {
            text: "history-199".into()
        })
    );
    events
        .send(HarnessEvent::Noticed {
            cursor: before.next(),
            text: "new-content".into(),
        })
        .expect("live");
    let event: HarnessEvent = reader.read().await.expect("live");
    assert_eq!(
        event.cursor(),
        before.next(),
        "historical events must not scroll across the screen"
    );
    server.abort();
}
