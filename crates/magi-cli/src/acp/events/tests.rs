#![allow(clippy::expect_used)]

use super::*;
use magi_proto::{AgentStatus, Cursor};
use tokio::io::AsyncWriteExt;

fn identity() -> Identity {
    Identity {
        external: "external".into(),
        native: SessionId::new("native"),
        generation: 7,
    }
}

fn event() -> HarnessEvent {
    HarnessEvent::StatusChanged {
        cursor: Cursor(9),
        status: AgentStatus::Idle,
    }
}

#[tokio::test(start_paused = true)]
async fn native_reader_defers_buffered_events_until_activation() {
    let (source, reader) = tokio::io::duplex(4096);
    let (send, mut receive) = mpsc::channel(1);
    let (failure, failed) = watch::channel(false);
    let mut task = Reader::start(reader, identity(), send, failure);
    let mut writer = magi_ipc::FrameWriter::new(source);
    writer.write(&event()).await.expect("buffered event");
    tokio::time::sleep(DEADLINE * 2).await;
    assert!(receive.try_recv().is_err());
    assert!(!*failed.borrow());
    task.activate();
    task.activate();
    let received = receive.recv().await.expect("activated event");
    assert!(received.identity == identity());
    assert_eq!(received.body, event());
    task.finish().await;
    assert!(!*failed.borrow());
}

#[tokio::test(start_paused = true)]
async fn abandoned_paused_reader_does_not_fail_other_sessions() {
    for finish in [true, false] {
        let (source, reader) = tokio::io::duplex(16);
        let (send, mut receive) = mpsc::channel(1);
        let (failure, failed) = watch::channel(false);
        let mut task = Reader::start(reader, identity(), send, failure);
        drop(source);
        tokio::time::sleep(DEADLINE * 2).await;
        assert!(!*failed.borrow());
        if finish {
            task.finish().await;
        }
        drop(task);
        assert!(receive.recv().await.is_none());
        assert!(!*failed.borrow());
    }
}

#[tokio::test(start_paused = true)]
async fn native_reader_allows_idle_and_expires_partial_frames() {
    let (mut source, reader) = tokio::io::duplex(128);
    let (send, _receive) = mpsc::channel(1);
    let (failure, mut failed) = watch::channel(false);
    let mut task = Reader::start(reader, identity(), send, failure);
    task.activate();
    tokio::time::sleep(DEADLINE * 2).await;
    assert!(!*failed.borrow());
    source.write_all(&[0]).await.expect("partial prefix");
    failed.changed().await.expect("partial deadline");
    assert!(*failed.borrow());
    task.finish().await;
}

#[tokio::test]
async fn native_reader_preserves_fragmented_frames_identity_and_order() {
    let (mut source, reader) = tokio::io::duplex(128);
    let (send, mut receive) = mpsc::channel(4);
    let (failure, failed) = watch::channel(false);
    let mut task = Reader::start(reader, identity(), send, failure);
    task.activate();
    let mut bytes = Vec::new();
    magi_ipc::FrameWriter::new(&mut bytes)
        .write(&event())
        .await
        .expect("frame");
    for _ in 0..2 {
        for byte in &bytes {
            source.write_all(&[*byte]).await.expect("fragment");
            tokio::task::yield_now().await;
        }
        let received = receive.recv().await.expect("event");
        assert_eq!(received.identity.external, "external");
        assert_eq!(received.identity.native, SessionId::new("native"));
        assert_eq!(received.identity.generation, 7);
        assert_eq!(received.body, event());
    }
    assert!(!*failed.borrow());
    task.finish().await;
    assert!(!*failed.borrow());
}

#[tokio::test(start_paused = true)]
async fn native_reader_backpressure_is_bounded_and_reports_failure() {
    let (source, reader) = tokio::io::duplex(4096);
    let (send, receive) = mpsc::channel(1);
    let (failure, mut failed) = watch::channel(false);
    let mut task = Reader::start(reader, identity(), send, failure);
    task.activate();
    let mut writer = magi_ipc::FrameWriter::new(source);
    for _ in 0..3 {
        writer.write(&event()).await.expect("frame");
    }
    failed.changed().await.expect("queue deadline");
    assert!(*failed.borrow());
    assert_eq!(receive.len(), 1);
    task.finish().await;
}
