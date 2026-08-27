//! The same framing, over blocking pipes.
//!
//! A tool runs synchronously — a Lua body is a function call, and making the trait async would
//! push a runtime into the VM's thread for no gain. A process tool talks over a child's stdin
//! and stdout, which are blocking handles, so the frames need a blocking reader and writer.
//!
//! Byte-for-byte identical to the async pair: a `u32` big-endian length, then that many bytes
//! of CBOR. A peer cannot tell which one it is talking to, which is the point — the same
//! extension works over a socket or a pipe.

use crate::IpcError;
use crate::codec::MAX_FRAME_BYTES;
use axum_proto::{Envelope, PROTOCOL_VERSION};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{Read, Write};

/// Reads framed messages from a blocking stream.
pub struct FrameReader<R> {
    inner: R,
    scratch: Vec<u8>,
}

impl<R: Read> FrameReader<R> {
    /// Wrap a readable stream.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            scratch: Vec::new(),
        }
    }

    /// Read one message.
    pub fn read_blocking<T: DeserializeOwned>(&mut self) -> Result<T, IpcError> {
        let mut len_bytes = [0_u8; 4];
        match self.inner.read_exact(&mut len_bytes) {
            Ok(()) => {}
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

/// Writes framed messages to a blocking stream.
pub struct FrameWriter<W> {
    inner: W,
    scratch: Vec<u8>,
}

impl<W: Write> FrameWriter<W> {
    /// Wrap a writable stream.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            scratch: Vec::new(),
        }
    }

    /// Write one message.
    pub fn write_blocking<T: Serialize>(&mut self, body: &T) -> Result<(), IpcError> {
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
        self.inner.write_all(&len_bytes)?;
        self.inner.write_all(&self.scratch)?;
        self.inner.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_proto::UiCommand;

    #[test]
    fn a_message_survives_a_round_trip() {
        let mut buffer = Vec::new();
        FrameWriter::new(&mut buffer)
            .write_blocking(&UiCommand::Interrupt)
            .expect("write");
        let received: UiCommand = FrameReader::new(buffer.as_slice())
            .read_blocking()
            .expect("read");
        assert_eq!(received, UiCommand::Interrupt);
    }

    #[test]
    fn the_framing_matches_the_async_pair_byte_for_byte() {
        // A peer must not be able to tell which side it is talking to.
        let mut blocking = Vec::new();
        FrameWriter::new(&mut blocking)
            .write_blocking(&UiCommand::Detach)
            .expect("write");

        let mut asynchronous = Vec::new();
        {
            let mut writer = crate::FrameWriter::new(&mut asynchronous);
            futures_lite_block_on(writer.write(&UiCommand::Detach)).expect("write");
        }
        assert_eq!(blocking, asynchronous);
    }

    /// Drive one future to completion without pulling in a runtime.
    fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut future = Box::pin(future);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    #[test]
    fn a_closed_stream_reports_disconnect() {
        let empty: &[u8] = &[];
        let error = FrameReader::new(empty)
            .read_blocking::<UiCommand>()
            .expect_err("must fail");
        assert!(matches!(error, IpcError::Disconnected));
    }

    #[test]
    fn an_oversized_length_is_refused_without_allocating() {
        let mut framed = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(b"x");
        let error = FrameReader::new(framed.as_slice())
            .read_blocking::<UiCommand>()
            .expect_err("must fail");
        assert!(matches!(error, IpcError::FrameTooLarge { .. }));
    }

    #[test]
    fn garbage_is_malformed_not_a_panic() {
        let body = [0xff_u8; 8];
        let mut framed = (body.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&body);
        let error = FrameReader::new(framed.as_slice())
            .read_blocking::<UiCommand>()
            .expect_err("must fail");
        assert!(matches!(error, IpcError::Malformed));
    }
}
