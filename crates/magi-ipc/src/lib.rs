//! Unix-socket transport for the magi wire contract.
//!
//! Frames are length-prefixed CBOR: a big-endian `u32` byte count followed by that many bytes
//! of `ciborium` encoding. Self-delimiting on the wire, so a peer that dies mid-frame is
//! caught at the length rather than misparsed deeper in.
//!
//! Linux only, deliberately. Peer identity comes from `SO_PEERCRED`, which means there is no
//! handshake token to design, issue, or leak.

pub mod blocking;
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
    runtime_dir.join("magi").join("host.sock")
}

/// The socket a daemon serving `cwd` listens on.
///
/// Per-directory rather than one path for the machine: `magi` in two repositories is two
/// conversations, and a shared socket would attach the second to the first's transcript
/// without either side noticing. The name is a digest because a directory path is not a
/// filename, and because a socket path has a length limit a deep tree would reach.
#[must_use]
pub fn socket_for(cwd: &Path) -> PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime_dir.join("magi").join(format!(
        "{:016x}.sock",
        digest(cwd.as_os_str().as_encoded_bytes())
    ))
}

/// FNV-1a, so the same directory names the same socket in every build.
///
/// Written out rather than taken from `DefaultHasher`, whose output is not promised to be
/// stable: a compiler upgrade would rename every socket and orphan any daemon still running.
fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
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
        assert!(path.ends_with("magi/host.sock"), "got {}", path.display());
    }
}

#[cfg(test)]
mod socket_tests {
    use super::*;

    #[test]
    fn two_directories_get_two_sockets() {
        let a = socket_for(Path::new("/home/x/one"));
        let b = socket_for(Path::new("/home/x/two"));
        assert_ne!(a, b);
    }

    #[test]
    fn the_same_directory_always_gets_the_same_socket() {
        assert_eq!(
            socket_for(Path::new("/home/x/one")),
            socket_for(Path::new("/home/x/one"))
        );
    }

    #[test]
    fn a_deep_path_still_names_a_short_socket() {
        // Unix socket paths cap around 108 bytes; a project path can exceed that on its own.
        let deep = Path::new("/home/x").join("a".repeat(300));
        let socket = socket_for(&deep);
        assert!(
            socket.file_name().is_some_and(|n| n.len() < 32),
            "{}",
            socket.display()
        );
    }
}
