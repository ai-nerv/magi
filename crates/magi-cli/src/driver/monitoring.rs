//! Bounded, read-only host probes while the agents tree is open.

use crate::app::App;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{UiCommand, monitoring::AgentDetails};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

type Sample = (String, PathBuf, Option<AgentDetails>);
const REFRESH: Duration = Duration::from_secs(2);
const PATIENCE: Duration = Duration::from_millis(750);

pub(super) struct Poller {
    next: Instant,
    task: Option<JoinHandle<()>>,
    send: mpsc::Sender<Vec<Sample>>,
    receive: mpsc::Receiver<Vec<Sample>>,
}

impl Poller {
    pub(super) fn new() -> Self {
        let (send, receive) = mpsc::channel(1);
        Self {
            next: Instant::now(),
            task: None,
            send,
            receive,
        }
    }

    pub(super) fn pump(&mut self, app: &mut App) {
        if app.pane.as_ref().is_none_or(|pane| pane.title != "agents") {
            if let Some(task) = self.task.take() {
                task.abort();
            }
            while self.receive.try_recv().is_ok() {}
            self.next = Instant::now();
            return;
        }
        while let Ok(batch) = self.receive.try_recv() {
            let now = Instant::now();
            for (id, socket, details) in batch {
                if app
                    .reachable
                    .iter()
                    .any(|peer| peer.id == id && peer.ui.as_ref() == Some(&socket))
                {
                    app.agent_details.insert(id, (socket, details, now));
                }
            }
            self.next = now + REFRESH;
        }
        app.agent_details.retain(|id, (socket, _, _)| {
            app.reachable
                .iter()
                .any(|peer| &peer.id == id && peer.ui.as_ref() == Some(socket))
        });
        if self.task.as_ref().is_some_and(JoinHandle::is_finished) {
            self.task = None;
        }
        if self.task.is_none() && Instant::now() >= self.next {
            let targets: Vec<_> = app
                .reachable
                .iter()
                .filter_map(|peer| peer.ui.clone().map(|socket| (peer.id.clone(), socket)))
                .collect();
            let send = self.send.clone();
            self.next = Instant::now() + REFRESH;
            self.task = Some(tokio::spawn(async move {
                let mut batch = Vec::new();
                for chunk in targets.chunks(8) {
                    let mut jobs = JoinSet::new();
                    for (id, socket) in chunk.iter().cloned() {
                        jobs.spawn(async move {
                            let details = probe(&socket).await;
                            (id, socket, details)
                        });
                    }
                    while let Some(Ok(sample)) = jobs.join_next().await {
                        batch.push(sample);
                    }
                }
                let _ = send.send(batch).await;
            }));
        }
        app.show_agents();
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn probe(socket: &Path) -> Option<AgentDetails> {
    tokio::time::timeout(PATIENCE, async {
        let stream = magi_ipc::connect(socket).await.ok()?;
        let (read, write) = stream.into_split();
        FrameWriter::new(write)
            .write(&UiCommand::Inspect)
            .await
            .ok()?;
        FrameReader::new(read).read::<AgentDetails>().await.ok()
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests;
