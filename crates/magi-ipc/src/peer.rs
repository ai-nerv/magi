//! Peer identity from the kernel.

use crate::IpcError;
use tokio::net::UnixStream;

/// Who is on the other end of a connection, as the kernel reports it. `SO_PEERCRED` cannot be
/// forged by the peer, so the daemon needs no handshake token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
    pub pid: Option<i32>,
    pub uid: u32,
    pub gid: u32,
}

impl PeerCred {
    /// Read the credentials of a connected peer.
    pub fn of(stream: &UnixStream) -> Result<Self, IpcError> {
        let cred = stream.peer_cred()?;
        Ok(Self {
            pid: cred.pid(),
            uid: cred.uid(),
            gid: cred.gid(),
        })
    }

    /// Whether this peer runs as the same user as the current process.
    #[must_use]
    pub fn is_same_user(self) -> bool {
        self.uid == rustix::process::getuid().as_raw()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_local_peer_is_the_same_user() {
        let (a, _b) = UnixStream::pair().expect("socketpair");
        let cred = PeerCred::of(&a).expect("peer_cred");
        assert!(cred.is_same_user());
        assert_eq!(cred.uid, rustix::process::getuid().as_raw());
    }
}
