use super::{Child, Input, Owner};
use std::{io, process::Command, time::Duration};
use tokio::io::AsyncWriteExt;

pub enum Writer {
    Pipe(tokio::process::ChildStdin),
    Socket(tokio::net::UnixStream),
}

impl Writer {
    pub async fn send(&mut self, body: &[u8]) -> io::Result<()> {
        match self {
            Self::Pipe(input) => {
                input.write_all(body).await?;
                input.shutdown().await
            }
            Self::Socket(input) => {
                input.write_all(body).await?;
                input.shutdown().await
            }
        }
    }
}

/// Own the native process through cancellation, reaping and event retirement.
pub struct Stream {
    pub input: Option<Writer>,
    pub stdout: Option<tokio::process::ChildStdout>,
    child: Child,
}

impl Stream {
    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub async fn kill(&mut self) -> io::Result<()> {
        self.child.kill()?;
        self.wait().await.map(|_| ())
    }
}

/// Negotiate outside the async executor and retain ownership if startup is cancelled.
pub async fn spawn(command: Command, owner: Owner) -> io::Result<Stream> {
    let lineage = crate::events::context::lineage();
    let (command, supported) = if super::CURRENT
        .get()
        .and_then(std::sync::Weak::upgrade)
        .is_some()
    {
        tokio::task::spawn_blocking(move || super::negotiate(command, owner))
            .await
            .map_err(io::Error::other)?
    } else {
        super::negotiate(command, owner)
    };
    let mut child = super::prepare(command, owner, supported, lineage).spawn()?;
    let input = match child.input.take() {
        Some(Input::Pipe(input)) => {
            Some(Writer::Pipe(tokio::process::ChildStdin::from_std(input)?))
        }
        Some(Input::Socket(input)) => {
            input.set_nonblocking(true)?;
            Some(Writer::Socket(tokio::net::UnixStream::from_std(input)?))
        }
        None => None,
    };
    let stdout = child
        .stdout
        .take()
        .map(tokio::process::ChildStdout::from_std)
        .transpose()?;
    Ok(Stream {
        input,
        stdout,
        child,
    })
}
