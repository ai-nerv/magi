//! Unix-socket transport for the magi wire contract. Frames are length-prefixed CBOR: a big-endian
//! `u32` byte count followed by that many bytes of `ciborium` encoding, so a peer that dies
//! mid-frame is caught at the length. Linux only; peer identity comes from `SO_PEERCRED`.

mod binding;
pub mod blocking;
mod codec;
mod encoding;
pub mod family;
mod peer;
mod runtime;
mod secret;

pub use binding::{clear_stale, exclusive, lock_path, unbind};
pub use codec::{FrameReader, FrameWriter, MAX_FRAME_BYTES};
pub use encoding::Wire;
pub use peer::PeerCred;
pub use runtime::{private_dir, private_dirs, runtime_base, runtime_dir, transient, trusted_xdg};
pub use secret::{drop_key, key_path, mint_key, random_hex, read_key, same_key};

use std::io;
use std::path::{Path, PathBuf};
use tokio::net::{UnixListener, UnixStream};

/// Anything that can go wrong on the transport.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] io::Error),

    /// A frame declared a length past [`MAX_FRAME_BYTES`], refused without allocating for it.
    #[error("frame of {len} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    FrameTooLarge { len: usize },

    /// The frame was not valid CBOR, or did not match the expected shape.
    #[error("malformed frame")]
    Malformed,

    #[error("peer speaks protocol version {peer}, this build speaks {ours}")]
    VersionMismatch { peer: u16, ours: u16 },

    #[error("peer disconnected")]
    Disconnected,
}

/// Where the daemon listens and the UI dials: magi's runtime directory — see [`runtime_base`].
#[must_use]
pub fn default_socket_path() -> PathBuf {
    runtime_base("magi").join("host.sock")
}

/// The socket a daemon serving `cwd` listens on. Per-directory, because `magi` in two repositories
/// is two conversations; the name is a digest because a socket path has a length limit.
#[must_use]
pub fn socket_for(cwd: &Path) -> PathBuf {
    runtime_base("magi").join(format!(
        "{:016x}.sock",
        digest(cwd.as_os_str().as_encoded_bytes())
    ))
}

/// FNV-1a, written out rather than taken from `DefaultHasher`, whose output is not stable across
/// builds: a compiler upgrade would rename every socket and orphan any daemon still running.
fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Bind a listener under an exclusive lock on `<socket>.lock`, held until [`unbind`]. A socket
/// already there is removed only when nothing answers it and it is this user's; anything else
/// there is a refusal. A fresh controller key goes beside it, in `<socket>.key`.
pub async fn bind(path: &Path) -> Result<UnixListener, IpcError> {
    if let Some(parent) = path.parent() {
        std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
            .create(parent)?;
    }
    let lock = binding::take(path)?;
    let listener = UnixListener::bind(path)?;
    binding::hold(path, lock)?;
    mint_key(path)?;
    Ok(listener)
}

/// Connect to a listening daemon.
pub async fn connect(path: &Path) -> Result<UnixStream, IpcError> {
    let stream = UnixStream::connect(path).await?;
    // A session is this user's; a socket another uid answers on is not one, whatever its name.
    if !PeerCred::of(&stream)?.is_same_user() {
        return Err(IpcError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is served by another user", path.display()),
        )));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_lives_under_a_runtime_dir() {
        let path = default_socket_path();
        assert_eq!(path, runtime_base("magi").join("host.sock"));
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
