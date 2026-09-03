//! Starting the memory layer, and taking it down again.
//!
//! magi convenes its siblings rather than finding them lying about. balthasar holds this
//! session's transcript, so a session that had to wait for somebody else to start one would be a
//! session that sometimes records and sometimes does not.
//!
//! **One balthasar per magi, named after the session.** Not one per project: two windows in a
//! project would then share an instance, and whichever quit first would take the other's store
//! out from under it — which is exactly how the old daemon failed. They still meet, but in the
//! project's store file rather than in a process.
//!
//! **It dies with its magi.** Nothing outlives the window here, and a memory layer left running
//! is the daemon pile in another costume. `PR_SET_PDEATHSIG` would be the airtight version and
//! needs `unsafe`, which this workspace denies; the child is killed on the way out instead, and
//! an orphan left by a kill -9 is swept by the next magi that looks.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// The child this process started, so it can be ended.
static STARTED: Mutex<Option<Child>> = Mutex::new(None);

/// How long to wait for a freshly started balthasar to bind.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// Start a balthasar for this session and return the socket it bound.
///
/// `None` when balthasar is not installed or did not bind in time, which is the ordinary case on
/// a machine without it: the session then keeps its own journal exactly as it did before.
pub async fn start(instance: &str, project: &Path) -> Option<PathBuf> {
    // Somebody else already said which one to talk to — a magi spawned by a balthasar, or a test
    // pointing at a fixture. Theirs, not ours to start.
    if std::env::var_os("MAGI_API_SOCKET").is_some_and(|v| !v.is_empty()) {
        return None;
    }

    let socket = magi_ipc::family::socket_dir().join(format!("api@{instance}.sock"));
    sweep(&socket);

    let child = Command::new("balthasar")
        .arg("serve")
        .arg("--instance")
        .arg(instance)
        .arg("--scope")
        .arg("project")
        .current_dir(project)
        // Silenced: this shares a terminal with the UI, and a line on stderr lands in the middle
        // of a frame.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Ok(mut held) = STARTED.lock() {
        *held = Some(child);
    }

    // Polled rather than assumed. A socket appears when balthasar binds it, and dialling before
    // then is the one failure that would look like "balthasar is not installed".
    let deadline = std::time::Instant::now() + PATIENCE;
    while std::time::Instant::now() < deadline {
        if magi_ipc::family::blocking::Family::dial(&socket).is_ok() {
            return Some(socket);
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    stop();
    None
}

/// End the balthasar this process started.
pub fn stop() {
    let Ok(mut held) = STARTED.lock() else {
        return;
    };
    let Some(mut child) = held.take() else {
        return;
    };
    let _ = child.kill();
    let _ = child.wait();
}

/// Clear a socket at `path` that nothing is serving.
///
/// Dialled, never guessed: unlinking a path because it looks stale would take a live balthasar's
/// socket out from under it. A session id is unique per magi, so a file already at this path was
/// left by a magi that died badly.
fn sweep(path: &Path) {
    if !path.exists() {
        return;
    }
    if magi_ipc::family::blocking::Family::dial(path).is_err() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_named_socket_is_not_taken_from_a_live_balthasar() {
        // The sweep must dial rather than stat. A listener here stands in for a live one.
        let dir = std::env::temp_dir().join(format!("magi-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("api@live.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");

        sweep(&path);
        assert!(path.exists(), "a socket something is serving must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_socket_nothing_answers_is_cleared() {
        let dir = std::env::temp_dir().join(format!("magi-sweep-dead-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("api@dead.sock");
        {
            let _listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        }

        sweep(&path);
        assert!(!path.exists(), "a socket nothing answers must be cleared");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_socket_somebody_else_named_is_not_ours_to_start() {
        // Set for the length of this test only, and read before anything is spawned.
        let saved = std::env::var_os("MAGI_API_SOCKET");
        assert!(
            saved.is_none() || start("x", Path::new("/tmp")).await.is_none(),
            "an explicit socket means somebody else's balthasar"
        );
    }
}
