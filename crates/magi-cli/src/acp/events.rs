use anyhow::{Result, bail};
use magi_ipc::FrameReader;
use magi_proto::{HarnessEvent, SessionId};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use super::transport::DEADLINE;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Identity {
    pub external: String,
    pub native: SessionId,
    pub generation: u64,
}

pub(super) struct Event {
    pub identity: Identity,
    pub body: HarnessEvent,
}

pub(super) struct Reader {
    task: Option<JoinHandle<()>>,
    ready: Option<oneshot::Sender<()>>,
}

impl Reader {
    pub(super) fn start(
        reader: impl AsyncRead + Unpin + Send + 'static,
        identity: Identity,
        sender: mpsc::Sender<Event>,
        failed: watch::Sender<bool>,
    ) -> Self {
        let (ready, activated) = oneshot::channel();
        let task = tokio::spawn(async move {
            if activated.await.is_err() {
                return;
            }
            let _ = read(reader, identity, sender).await;
            failed.send_replace(true);
        });
        Self {
            task: Some(task),
            ready: Some(ready),
        }
    }

    pub(super) fn activate(&mut self) {
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(());
        }
    }

    pub(super) async fn finish(&mut self) {
        self.ready = None;
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn read(
    reader: impl AsyncRead + Unpin,
    identity: Identity,
    sender: mpsc::Sender<Event>,
) -> Result<()> {
    let mut reader = BufReader::with_capacity(8192, reader);
    loop {
        if reader.fill_buf().await?.is_empty() {
            bail!("native event stream closed");
        }
        let body = tokio::time::timeout(
            DEADLINE,
            FrameReader::new(&mut reader).read::<HarnessEvent>(),
        )
        .await??;
        tokio::time::timeout(
            DEADLINE,
            sender.send(Event {
                identity: identity.clone(),
                body,
            }),
        )
        .await??;
    }
}

#[cfg(test)]
mod tests;
