use super::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncReadExt, DuplexStream},
    sync::oneshot,
};

struct HeldFlush {
    output: DuplexStream,
    release: oneshot::Receiver<()>,
}

impl AsyncWrite for HeldFlush {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.output).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.release).poll(cx).map(|_| Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn reply_newline_releases_id_before_flush_and_old_drop_preserves_reuse() {
    let (mut client, input) = tokio::io::duplex(1024);
    let mut intake = super::super::intake::Intake::new(input);
    let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
    client.write_all(request).await.expect("first request");
    let mut item = intake.incoming.recv().await.expect("first admission");
    let (output, response) = tokio::io::duplex(1024);
    let (release, held) = oneshot::channel();
    let writing = tokio::spawn(async move {
        let mut output = HeldFlush {
            output,
            release: held,
        };
        reply(
            &mut output,
            &serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}}),
            &mut item,
        )
        .await
        .expect("reply");
    });
    let mut response = tokio::io::BufReader::new(response);
    let mut bytes = Vec::new();
    response
        .read_until(b'\n', &mut bytes)
        .await
        .expect("visible response");
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert!(!writing.is_finished(), "flush still held");
    client
        .write_all(request)
        .await
        .expect("reuse after response");
    let reused = tokio::time::timeout(Duration::from_secs(2), intake.incoming.recv())
        .await
        .expect("bounded admission")
        .expect("completed ID reusable before flush");
    release.send(()).expect("release flush");
    writing.await.expect("reply task");
    client
        .write_all(request)
        .await
        .expect("duplicate while reuse pending");
    assert!(intake.incoming.recv().await.is_none());
    assert!(intake.ended().await.is_err());
    drop(reused);
    intake.finish().await;
}

#[tokio::test]
async fn partial_reply_keeps_request_id_pending() {
    let (mut client, input) = tokio::io::duplex(1024);
    let mut intake = super::super::intake::Intake::new(input);
    let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
    client.write_all(request).await.expect("first request");
    let mut item = intake.incoming.recv().await.expect("admitted");
    let (mut output, mut response) = tokio::io::duplex(1);
    let writing = tokio::spawn(async move {
        reply(
            &mut output,
            &serde_json::json!({"id":1,"result":{}}),
            &mut item,
        )
        .await
    });
    assert_eq!(response.read_u8().await.expect("partial response"), b'{');
    client
        .write_all(request)
        .await
        .expect("premature duplicate");
    assert!(intake.incoming.recv().await.is_none());
    assert!(intake.ended().await.is_err());
    writing.abort();
    let _ = writing.await;
    intake.finish().await;
}

enum End {
    Block,
    Zero,
    Fail,
}

struct HeldEnd {
    output: DuplexStream,
    reached: Option<oneshot::Sender<()>>,
    end: End,
}

impl AsyncWrite for HeldEnd {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes == b"\n" {
            if let Some(reached) = self.reached.take() {
                let _ = reached.send(());
            }
            return match self.end {
                End::Block => Poll::Pending,
                End::Zero => Poll::Ready(Ok(0)),
                End::Fail => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            };
        }
        Pin::new(&mut self.output).poll_write(cx, bytes)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn blocked_newline_keeps_request_id_pending() {
    let (mut client, input) = tokio::io::duplex(1024);
    let mut intake = super::super::intake::Intake::new(input);
    let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
    client.write_all(request).await.expect("first request");
    let mut item = intake.incoming.recv().await.expect("admitted");
    let (output, _response) = tokio::io::duplex(1024);
    let (reached, waiting) = oneshot::channel();
    let writing = tokio::spawn(async move {
        let mut output = HeldEnd {
            output,
            reached: Some(reached),
            end: End::Block,
        };
        reply(
            &mut output,
            &serde_json::json!({"id":1,"result":{}}),
            &mut item,
        )
        .await
    });
    waiting.await.expect("newline blocked");
    client
        .write_all(request)
        .await
        .expect("premature duplicate");
    assert!(intake.incoming.recv().await.is_none());
    assert!(intake.ended().await.is_err());
    writing.abort();
    let _ = writing.await;
    intake.finish().await;
}

#[tokio::test]
async fn failed_newline_does_not_retire_a_live_request() {
    for end in [End::Zero, End::Fail] {
        let (mut client, input) = tokio::io::duplex(1024);
        let mut intake = super::super::intake::Intake::new(input);
        let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
        client.write_all(request).await.expect("first request");
        let mut item = intake.incoming.recv().await.expect("admitted");
        let (output, _response) = tokio::io::duplex(1024);
        let mut output = HeldEnd {
            output,
            reached: None,
            end,
        };
        assert!(
            reply(
                &mut output,
                &serde_json::json!({"id":1,"result":{}}),
                &mut item
            )
            .await
            .is_err()
        );
        client
            .write_all(request)
            .await
            .expect("duplicate after failed output");
        assert!(intake.incoming.recv().await.is_none());
        assert!(intake.ended().await.is_err());
        drop(item);
        intake.finish().await;
    }
}
