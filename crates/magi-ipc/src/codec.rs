//! Length-prefixed CBOR framing.

use crate::IpcError;
use magi_proto::{Envelope, PROTOCOL_VERSION};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest frame either side will read or write.
///
/// Tau caps at 16 MiB and every collection it owns has a named limit. A transcript event is
/// kilobytes; anything approaching this is a bug or an attack, and either way the connection
/// should die rather than the process.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Reads framed messages from one half of a connection.
pub struct FrameReader<R> {
    inner: R,
    /// Reused across frames so a busy stream does not reallocate per message.
    scratch: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// Wrap the read half of a connection.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            scratch: Vec::new(),
        }
    }

    /// Read one message.
    ///
    /// The version is checked before the body is handed back, so a peer from another build is
    /// rejected at the boundary rather than after its fields have been interpreted.
    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<T, IpcError> {
        let mut len_bytes = [0_u8; 4];
        match self.inner.read_exact(&mut len_bytes).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(IpcError::Disconnected);
            }
            Err(e) => return Err(IpcError::Io(e)),
        }

        let len = u32::from_be_bytes(len_bytes) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(IpcError::FrameTooLarge { len });
        }

        self.scratch.clear();
        self.scratch.resize(len, 0);
        self.inner
            .read_exact(&mut self.scratch)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => IpcError::Disconnected,
                _ => IpcError::Io(e),
            })?;

        let envelope: Envelope<T> =
            ciborium::from_reader(self.scratch.as_slice()).map_err(|_| IpcError::Malformed)?;
        if envelope.version != PROTOCOL_VERSION {
            return Err(IpcError::VersionMismatch {
                peer: envelope.version,
                ours: PROTOCOL_VERSION,
            });
        }
        Ok(envelope.body)
    }
}

/// Writes framed messages to one half of a connection.
pub struct FrameWriter<W> {
    inner: W,
    scratch: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    /// Wrap the write half of a connection.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            scratch: Vec::new(),
        }
    }

    /// Write one message.
    ///
    /// Encoding happens into scratch first so an oversized frame is refused before any of it
    /// reaches the socket — a partial write would desynchronize the stream permanently.
    pub async fn write<T: Serialize>(&mut self, body: &T) -> Result<(), IpcError> {
        self.scratch.clear();
        let envelope = Envelope {
            version: PROTOCOL_VERSION,
            body,
        };
        ciborium::into_writer(&envelope, &mut self.scratch).map_err(|_| IpcError::Malformed)?;

        let len = self.scratch.len();
        if len > MAX_FRAME_BYTES {
            return Err(IpcError::FrameTooLarge { len });
        }

        let len_bytes = u32::try_from(len)
            .map_err(|_| IpcError::FrameTooLarge { len })?
            .to_be_bytes();
        self.inner.write_all(&len_bytes).await?;
        self.inner.write_all(&self.scratch).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::UiCommand;

    #[tokio::test]
    async fn a_message_survives_a_round_trip() {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = FrameWriter::new(client);
        let mut reader = FrameReader::new(server);

        let sent = UiCommand::SubmitPrompt {
            aside: String::new(),
            text: "hello".into(),
        };
        writer.write(&sent).await.expect("write");
        let received: UiCommand = reader.read().await.expect("read");
        assert_eq!(sent, received);
    }

    #[tokio::test]
    async fn back_to_back_frames_stay_aligned() {
        let (client, server) = tokio::io::duplex(4096);
        let mut writer = FrameWriter::new(client);
        let mut reader = FrameReader::new(server);

        writer.write(&UiCommand::Interrupt).await.expect("write 1");
        writer.write(&UiCommand::Detach).await.expect("write 2");

        assert_eq!(
            reader.read::<UiCommand>().await.expect("read 1"),
            UiCommand::Interrupt
        );
        assert_eq!(
            reader.read::<UiCommand>().await.expect("read 2"),
            UiCommand::Detach
        );
    }

    #[tokio::test]
    async fn a_closed_peer_reports_disconnect_not_io() {
        let (client, server) = tokio::io::duplex(64);
        drop(client);
        let mut reader = FrameReader::new(server);
        assert!(matches!(
            reader.read::<UiCommand>().await,
            Err(IpcError::Disconnected)
        ));
    }

    #[tokio::test]
    async fn an_oversized_length_is_refused_without_allocating() {
        let (mut client, server) = tokio::io::duplex(64);
        let declared = MAX_FRAME_BYTES + 1;
        client
            .write_all(&(declared as u32).to_be_bytes())
            .await
            .expect("write length");
        let mut reader = FrameReader::new(server);
        assert!(matches!(
            reader.read::<UiCommand>().await,
            Err(IpcError::FrameTooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn garbage_is_malformed_not_a_panic() {
        let (mut client, server) = tokio::io::duplex(64);
        let body = [0xff_u8; 8];
        client
            .write_all(&(body.len() as u32).to_be_bytes())
            .await
            .expect("write length");
        client.write_all(&body).await.expect("write body");
        let mut reader = FrameReader::new(server);
        assert!(matches!(
            reader.read::<UiCommand>().await,
            Err(IpcError::Malformed)
        ));
    }
}
