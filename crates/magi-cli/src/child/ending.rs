use std::{future::Future, path::Path, pin::Pin};

use anyhow::Result;
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{AgentStatus, HarnessEvent, UiCommand};

pub(super) fn signal() -> std::io::Result<Pin<Box<dyn Future<Output = ()> + Send>>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    Ok(Box::pin(async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }))
}

pub(super) async fn settle_owned(socket: &Path) -> Result<()> {
    let stream = magi_ipc::connect(socket).await?;
    let (read, write) = stream.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::AttachLatest {
            draws: false,
            key: magi_ipc::read_key(socket),
        })
        .await?;
    if !matches!(
        reader.read::<HarnessEvent>().await?,
        HarnessEvent::SessionSnapshot { .. }
    ) {
        anyhow::bail!("native shutdown snapshot unavailable");
    }
    writer.write(&UiCommand::Interrupt).await?;
    loop {
        if matches!(
            reader.read::<HarnessEvent>().await?,
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            } | HarnessEvent::Refused { .. }
        ) {
            return Ok(());
        }
    }
}
