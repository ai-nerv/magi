use std::io::{self, Write};
use std::time::Duration;

use magi_ipc::MAX_FRAME_BYTES;
use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const DEADLINE: Duration = Duration::from_secs(30);

pub(super) async fn read(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    if reader.fill_buf().await?.is_empty() {
        return Ok(None);
    }
    tokio::time::timeout(DEADLINE, line(reader)).await?
}

async fn line(reader: &mut (impl AsyncBufRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unterminated ACP frame",
            ));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let length = newline.unwrap_or(available.len());
        if length > MAX_FRAME_BYTES.saturating_sub(bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ACP frame exceeds byte limit",
            ));
        }
        bytes.extend_from_slice(
            available
                .get(..length)
                .ok_or_else(|| io::Error::other("ACP frame bounds"))?,
        );
        reader.consume(length + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(bytes));
        }
    }
}

struct Encoded(Vec<u8>);

impl Write for Encoded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ACP response exceeds byte limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
) -> io::Result<()> {
    send(writer, value, None).await
}

async fn send(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    item: Option<&mut super::intake::Item>,
) -> io::Result<()> {
    let mut bytes = Encoded(Vec::new());
    serde_json::to_writer(&mut bytes, value)?;
    tokio::time::timeout(DEADLINE, async {
        writer.write_all(&bytes.0).await?;
        if let Some(item) = item {
            let size = std::future::poll_fn(|cx| item.poll_end(writer, cx)).await?;
            if size != 1 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "ACP frame terminator was not written",
                ));
            }
        } else {
            writer.write_all(b"\n").await?;
        }
        writer.flush().await
    })
    .await?
}

pub(super) async fn reply(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    item: &mut super::intake::Item,
) -> io::Result<()> {
    send(writer, value, Some(item)).await
}

#[cfg(test)]
mod replies;
