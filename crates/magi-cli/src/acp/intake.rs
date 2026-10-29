use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use agent_client_protocol_schema::v1::{Error, RequestId};
use anyhow::{Result, bail};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::{mpsc, watch};

use super::{envelope, json, transport};

type Pending = Arc<Mutex<HashSet<RequestId>>>;

pub(super) struct Item {
    pub message: Result<envelope::Incoming, Value>,
    _lease: Option<Lease>,
}

struct Lease {
    id: RequestId,
    pending: Pending,
    released: bool,
}

impl Item {
    pub(super) fn poll_end(
        &mut self,
        writer: &mut (impl AsyncWrite + Unpin),
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<usize>> {
        let Some(lease) = self._lease.as_mut() else {
            return Pin::new(writer).poll_write(cx, b"\n");
        };
        let mut held = lease
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = Pin::new(writer).poll_write(cx, b"\n");
        if matches!(result, Poll::Ready(Ok(1))) && !lease.released {
            held.remove(&lease.id);
            lease.released = true;
        }
        result
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

pub(super) struct Intake {
    pub incoming: mpsc::Receiver<Item>,
    pub closed: watch::Receiver<bool>,
    reading: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl Intake {
    pub(super) fn new(reader: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let (send, incoming) = mpsc::channel(2);
        let (closed, closing) = watch::channel(false);
        let reading = tokio::spawn(async move {
            let result = read(reader, send).await;
            closed.send_replace(true);
            result
        });
        Self {
            incoming,
            closed: closing,
            reading: Some(reading),
        }
    }

    pub(super) async fn finish(&mut self) {
        if let Some(reading) = self.reading.take() {
            reading.abort();
            let _ = reading.await;
        }
    }

    pub(super) async fn ended(&mut self) -> Result<()> {
        if let Some(reading) = self.reading.take() {
            reading.await??;
        }
        Ok(())
    }
}

impl Drop for Intake {
    fn drop(&mut self) {
        if let Some(reading) = &self.reading {
            reading.abort();
        }
    }
}

async fn read(reader: impl AsyncRead + Unpin, send: mpsc::Sender<Item>) -> Result<()> {
    let mut reader = BufReader::with_capacity(8192, reader);
    let pending: Pending = Arc::default();
    while let Some(bytes) = transport::read(&mut reader).await? {
        let message = match json::parse(&bytes) {
            Err(_) => Err(envelope::failure(None, Error::parse_error())),
            Ok(value) => envelope::parse(value)
                .map_err(|()| envelope::failure(None, Error::invalid_request())),
        };
        let lease = if let Ok(envelope::Incoming::Request { id, .. }) = &message {
            let mut held = pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if held.len() >= 4 || !held.insert(id.clone()) {
                bail!("ACP pending request collision or limit");
            }
            Some(Lease {
                id: id.clone(),
                pending: Arc::clone(&pending),
                released: false,
            })
        } else {
            None
        };
        tokio::time::timeout(
            transport::DEADLINE,
            send.send(Item {
                message,
                _lease: lease,
            }),
        )
        .await?
        .map_err(|_| anyhow::anyhow!("ACP receiver closed"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn repeated_pending_id_closes_without_an_ambiguous_second_reply() {
        let (mut client, input) = tokio::io::duplex(1024);
        let mut intake = Intake::new(input);
        let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
        client.write_all(request).await.expect("first request");
        let first = intake.incoming.recv().await.expect("first admission");
        client.write_all(request).await.expect("duplicate");
        assert!(intake.incoming.recv().await.is_none());
        assert!(intake.ended().await.is_err());
        assert!(*intake.closed.borrow());
        drop(first);
        intake.finish().await;
    }

    #[tokio::test]
    async fn completed_id_is_released_only_after_its_item_is_dropped() {
        let (mut client, input) = tokio::io::duplex(1024);
        let mut intake = Intake::new(input);
        let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
        client.write_all(request).await.expect("first");
        drop(intake.incoming.recv().await.expect("admitted"));
        client.write_all(request).await.expect("reuse");
        assert!(intake.incoming.recv().await.is_some());
        drop(client);
        assert!(intake.incoming.recv().await.is_none());
        intake.ended().await.expect("clean EOF");
        intake.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn bounded_admission_queue_expires_when_not_drained() {
        let (mut client, input) = tokio::io::duplex(1024);
        let mut intake = Intake::new(input);
        for id in 0..4 {
            client
                .write_all(
                    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"x\"}}\n").as_bytes(),
                )
                .await
                .expect("request");
        }
        intake.closed.changed().await.expect("deadline closes");
        assert!(intake.ended().await.is_err());
        assert_eq!(intake.incoming.len(), 2);
        intake.finish().await;
    }
}
