//! Unix-socket transport for the axum wire contract.
//!
//! Frames are length-prefixed CBOR: a big-endian `u32` byte count followed by that many bytes
//! of `ciborium` encoding. Self-delimiting on the wire, so a peer that dies mid-frame is
//! caught at the length rather than misparsed deeper in.
//!
//! Linux only, deliberately. Peer identity comes from `SO_PEERCRED`, which means there is no
//! handshake token to design, issue, or leak.

mod codec;
mod peer;

pub use codec::{FrameReader, FrameWriter, MAX_FRAME_BYTES};
pub use peer::PeerCred;

use std::io;
use std::path::{Path, PathBuf};
use tokio::net::{UnixListener, UnixStream};

/// Anything that can go wrong on the transport.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// The socket or filesystem operation failed.
    #[error("io: {0}")]
    Io(#[from] io::Error),

    /// A frame declared a length past [`MAX_FRAME_BYTES`].
    ///
    /// Refused without allocating for it: an oversized declaration is exactly how a hostile
    /// or confused peer would try to exhaust the reader.
    #[error("frame of {len} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    FrameTooLarge {
        /// The length the peer declared.
        len: usize,
    },

    /// The frame was not valid CBOR, or did not match the expected shape.
    ///
    /// The offending bytes are deliberately absent: a decode failure is when a payload is
    /// least trustworthy and most likely to reach a log.
    #[error("malformed frame")]
    Malformed,

    /// The peer speaks a protocol version this build does not.
    #[error("peer speaks protocol version {peer}, this build speaks {ours}")]
    VersionMismatch {
        /// The version the peer stamped on its frame.
        peer: u16,
        /// The version this build writes.
        ours: u16,
    },

    /// The peer closed the connection.
    #[error("peer disconnected")]
    Disconnected,
}

/// Where the daemon listens and the UI dials.
///
/// `$XDG_RUNTIME_DIR` is per-user and cleared on logout, which is what a socket wants; the
/// temp-dir fallback keeps things working where it is unset.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime_dir.join("axum").join("host.sock")
}

/// Bind a listener, clearing a stale socket left by a previous run.
///
/// A dead socket file makes `bind` fail with `EADDRINUSE` even though nothing is listening, so
/// an unconnectable path is removed first. A path that still answers is left alone and the bind
/// error surfaces, because that is a live daemon and not ours to evict.
pub async fn bind(path: &Path) -> Result<UnixListener, IpcError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::metadata(path).await.is_ok() && UnixStream::connect(path).await.is_err() {
        tokio::fs::remove_file(path).await?;
    }
    Ok(UnixListener::bind(path)?)
}

/// Connect to a listening daemon.
pub async fn connect(path: &Path) -> Result<UnixStream, IpcError> {
    Ok(UnixStream::connect(path).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_lives_under_a_runtime_dir() {
        let path = default_socket_path();
        assert!(path.ends_with("axum/host.sock"), "got {}", path.display());
    }
}
