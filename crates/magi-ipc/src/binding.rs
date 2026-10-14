//! Taking a socket path and giving it back. A path is taken under an exclusive `flock` on
//! `<socket>.lock`, held for as long as the listener is; an old socket there is removed only when
//! connecting to it is refused, it is a socket of this user's, and nobody holds its lock.

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The lock and the inode of every socket this process has bound and not yet given back.
static HELD: Mutex<Option<HashMap<PathBuf, (std::fs::File, u64)>>> = Mutex::new(None);

/// Where the lock for the socket at `path` lives.
#[must_use]
pub fn lock_path(path: &Path) -> PathBuf {
    let mut named = path.as_os_str().to_owned();
    named.push(".lock");
    PathBuf::from(named)
}

/// Take the lock for `path`, without waiting: `None` when another process holds it.
fn locked(path: &Path) -> io::Result<Option<std::fs::File>> {
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(lock_path(path))?;
    match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(lock)),
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
        Err(why) => Err(why.into()),
    }
}

/// Whether connecting to `path` is refused outright: nothing is listening there. A full backlog,
/// a timeout or any other error is a live program having a bad moment.
fn refused(path: &Path) -> bool {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let Ok(socket) = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    ) else {
        return false;
    };
    let Ok(address) = SocketAddrUnix::new(path) else {
        return false;
    };
    matches!(
        rustix::net::connect_unix(&socket, &address),
        Err(rustix::io::Errno::CONNREFUSED)
    )
}

/// Whether `path` is a socket owned by this user.
fn ours(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|held| {
        held.file_type().is_socket() && held.uid() == rustix::process::getuid().as_raw()
    })
}

/// Remove the socket at `path` when nothing serves it, and say whether it went. Its key goes with
/// it; a socket somebody holds the lock of, or that answers in any way, stays.
///
/// # Errors
/// When the lock file cannot be opened.
pub fn clear_stale(path: &Path) -> io::Result<bool> {
    // A lock file made only to ask is not left beside a socket that turns out to be alive.
    let made = std::fs::symlink_metadata(lock_path(path)).is_err();
    let Some(_lock) = locked(path)? else {
        return Ok(false);
    };
    let stale = ours(path) && refused(path);
    if stale {
        std::fs::remove_file(path)?;
        crate::secret::drop_key(path);
    }
    if stale || made {
        let _ = std::fs::remove_file(lock_path(path));
    }
    Ok(stale)
}

/// Take `path` for a listener this process is about to bind: its lock, held until [`unbind`], and
/// the path itself cleared when what is there is stale.
pub(crate) fn take(path: &Path) -> io::Result<std::fs::File> {
    let lock = locked(path)?.ok_or_else(|| in_use(path, "another process holds its lock"))?;
    if std::fs::symlink_metadata(path).is_ok() {
        if !(ours(path) && refused(path)) {
            return Err(in_use(
                path,
                "something is there and it is not a stale socket",
            ));
        }
        std::fs::remove_file(path)?;
    }
    Ok(lock)
}

/// Remember that `path` is bound, holding `lock`.
pub(crate) fn hold(path: &Path, lock: std::fs::File) -> io::Result<()> {
    let inode = std::fs::symlink_metadata(path)?.ino();
    if let Ok(mut held) = HELD.lock() {
        held.get_or_insert_with(HashMap::new)
            .insert(path.to_path_buf(), (lock, inode));
    }
    Ok(())
}

/// Give back a socket this process bound: unlinked only while it is still the inode that was
/// bound, with its key and its lock.
pub fn unbind(path: &Path) {
    let taken = HELD
        .lock()
        .ok()
        .and_then(|mut held| held.as_mut()?.remove(path));
    let Some((lock, inode)) = taken else {
        return;
    };
    if std::fs::symlink_metadata(path).is_ok_and(|now| now.ino() == inode) {
        let _ = std::fs::remove_file(path);
        crate::secret::drop_key(path);
    }
    let _ = std::fs::remove_file(lock_path(path));
    drop(lock);
}

/// Wait for an exclusive `flock` on `file`, held until it is closed.
///
/// # Errors
/// When the lock cannot be taken.
pub fn exclusive(file: &std::fs::File) -> io::Result<()> {
    rustix::fs::flock(file, rustix::fs::FlockOperation::LockExclusive).map_err(Into::into)
}

fn in_use(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AddrInUse,
        format!("{} is in use: {why}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_nothing_listens_on_is_cleared_and_a_live_one_is_not() {
        let dir = magi_model::scratch::Scratch::new("magi-binding", "stale");
        let dead = dir.join("dead.sock");
        drop(std::os::unix::net::UnixListener::bind(&dead).expect("bind"));
        let live = dir.join("live.sock");
        let _listening = std::os::unix::net::UnixListener::bind(&live).expect("bind");
        let plain = dir.join("plain.sock");
        std::fs::write(&plain, b"not a socket").expect("write");

        assert!(clear_stale(&dead).expect("swept"), "a corpse stayed");
        assert!(!dead.exists());
        assert!(!clear_stale(&live).expect("swept"), "a live socket went");
        assert!(
            !clear_stale(&plain).expect("swept"),
            "a file that is no socket went"
        );
        assert!(live.exists() && plain.exists());
        assert!(
            !lock_path(&live).exists() && !lock_path(&plain).exists(),
            "a lock file made to ask was left beside what it asked about"
        );
    }

    #[test]
    fn a_socket_whose_lock_is_held_is_left_alone() {
        let dir = magi_model::scratch::Scratch::new("magi-binding", "held");
        let dead = dir.join("held.sock");
        drop(std::os::unix::net::UnixListener::bind(&dead).expect("bind"));
        let _held = locked(&dead).expect("lock").expect("free");
        let other = std::fs::OpenOptions::new()
            .read(true)
            .open(lock_path(&dead))
            .expect("open");
        assert!(
            rustix::fs::flock(&other, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .is_err(),
            "the fixture's lock is not exclusive"
        );
        assert!(
            !clear_stale(&dead).expect("swept"),
            "the holder's socket went"
        );
        assert!(dead.exists());
    }

    #[tokio::test]
    async fn a_path_another_binder_holds_is_refused_and_given_back_cleanly() {
        let dir = magi_model::scratch::Scratch::new("magi-binding", "twice");
        let path = dir.join("s.sock");
        let first = crate::bind(&path).await.expect("first");
        let second = crate::bind(&path).await.expect_err("second");
        assert!(second.to_string().contains("in use"), "{second}");
        drop(first);
        unbind(&path);
        assert!(!path.exists() && !lock_path(&path).exists());
        assert!(!crate::key_path(&path).exists());
        let _again = crate::bind(&path).await.expect("free again");
        unbind(&path);
    }

    #[tokio::test]
    async fn a_socket_is_never_bound_where_others_may_enter() {
        use std::os::unix::fs::PermissionsExt;
        let dir = magi_model::scratch::Scratch::new("magi-binding", "open");
        let open = dir.join("open");
        std::fs::create_dir(&open).expect("mkdir");
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let refused = crate::bind(&open.join("s.sock"))
            .await
            .expect_err("refused");
        assert!(refused.to_string().contains("private"), "{refused}");
        assert!(!open.join("s.sock").exists());
        let made = dir.join("made/deeper");
        let _bound = crate::bind(&made.join("s.sock"))
            .await
            .expect("made private");
        let mode = std::fs::metadata(&made)
            .expect("there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        unbind(&made.join("s.sock"));
    }

    #[tokio::test]
    async fn giving_back_a_path_somebody_rebound_leaves_theirs() {
        let dir = magi_model::scratch::Scratch::new("magi-binding", "rebound");
        let path = dir.join("s.sock");
        let ours = crate::bind(&path).await.expect("bind");
        drop(ours);
        std::fs::remove_file(&path).expect("unlink");
        let _theirs = std::os::unix::net::UnixListener::bind(&path).expect("theirs");
        unbind(&path);
        assert!(
            path.exists(),
            "a socket this process did not bind was removed"
        );
    }
}
