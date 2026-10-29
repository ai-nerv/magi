use anyhow::{Context, Result, ensure};
use magi_proto::Cursor;
use serde_json::Value;
use std::path::Path;
use tokio::{
    io::AsyncWrite,
    sync::{mpsc, oneshot},
    time::Instant,
};

use crate::acp::{display, events, transport, turns};

pub(in crate::acp) struct Delivery {
    pub generation: u64,
    frame: Value,
    deadline: Instant,
    accepted: oneshot::Sender<()>,
}

impl Delivery {
    pub(in crate::acp) async fn write(
        mut self,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        if self.accepted.is_closed() || Instant::now() >= self.deadline {
            return Ok(());
        }
        tokio::select! {
            _ = self.accepted.closed() => anyhow::bail!("history replay abandoned during output"),
            result = tokio::time::timeout_at(self.deadline, transport::write(output, &self.frame)) => result??,
        }
        let _ = self.accepted.send(());
        Ok(())
    }
}

pub(in crate::acp) struct Replay {
    pub generation: u64,
    pub policy: display::Policy,
    pub output: mpsc::Sender<Delivery>,
}

impl Replay {
    pub(in crate::acp) async fn run(
        &self,
        socket: &Path,
        key: &str,
        identity: &events::Identity,
        through: Cursor,
    ) -> Result<()> {
        ensure!(
            identity.generation == self.generation,
            "history replay generation changed"
        );
        let transcript = super::native(&identity.external)?;
        let mut pages = super::paging::Paging::new(
            identity.native.clone(),
            through,
            Cursor::ZERO,
            Some(transcript),
        )?;
        let mut turn = turns::Turn::background();
        while let Some(page) = pages.read(socket, key).await? {
            for (cursor, entry) in page.entries {
                for event in magi_host::session::events_for(cursor, &entry) {
                    if let Some(frame) = self.policy.boundary(&identity.external, &event) {
                        self.send(frame).await?;
                    } else {
                        let update = turn.event(&identity.external, event)?;
                        ensure!(
                            !update.interrupt && update.complete.is_none(),
                            "history replay attempted a live action"
                        );
                        for frame in update.frames {
                            self.send(frame).await?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn send(&self, frame: Value) -> Result<()> {
        let deadline = Instant::now() + transport::DEADLINE;
        let (accepted, receipt) = oneshot::channel();
        tokio::time::timeout_at(deadline, async {
            self.output
                .send(Delivery {
                    generation: self.generation,
                    frame,
                    deadline,
                    accepted,
                })
                .await
                .map_err(|_| anyhow::anyhow!("history replay output closed"))?;
            receipt
                .await
                .context("history replay output was not acknowledged")
        })
        .await
        .context("history replay output deadline")??;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
