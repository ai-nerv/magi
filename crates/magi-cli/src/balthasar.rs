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

/// The balthasar this process started, so it can be ended and its path cleared.
static STARTED: Mutex<Option<Ours>> = Mutex::new(None);

/// A balthasar this magi started, and the path it was told to bind.
///
/// The path is kept beside the child because only the two together can be tidied up: the child
/// is stopped with a signal it cannot handle, so it never unlinks its own socket, and the path
/// alone is not enough to know whether unlinking it is safe.
struct Ours {
    /// The process, to end.
    child: Child,
    /// Where it was told to listen, to unlink once it has.
    socket: PathBuf,
}

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

    let dir = magi_ipc::family::socket_dir();
    let socket = dir.join(format!("api@{instance}.sock"));
    // The whole directory, not only the path about to be taken. A session id is unique per
    // magi, so sweeping one path only ever cleared a corpse this same session had left --
    // which, since the id is never reused, is none. Every run therefore left a file behind for
    // good, and after a week the directory is a list of sessions that ended.
    sweep_stale(&dir);

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
        *held = Some(Ours {
            child,
            socket: socket.clone(),
        });
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

/// End the balthasar this process started, and clear the path it was listening on.
///
/// Reaped before the socket is unlinked, and in that order. The rule everywhere else here is
/// that a path is only ever removed once something has proved nothing is serving it — a dial
/// that was refused, for [`sweep`]. This is the other proof, and the stronger one: after
/// [`std::process::Child::wait`] the process is gone, so the name cannot still be answering.
/// Unlinking first would remove a name a live balthasar was serving on, which is the mistake
/// `sweep` exists to avoid.
///
/// Leaving it would not break anything — the next magi sweeps it — but "nothing outlives the
/// window" should be true of the file as well as the process.
pub fn stop() {
    let Ok(mut held) = STARTED.lock() else {
        return;
    };
    let Some(ours) = held.take() else {
        return;
    };
    ended(ours);
}

/// Stop one balthasar and clear the path it was listening on.
///
/// Split from [`stop`] so the order can be tested without the static, which is process-wide and
/// would make two tests that used it pass alone and fail together.
fn ended(Ours { mut child, socket }: Ours) {
    let _ = child.kill();
    let _ = child.wait();
    // Absent when it never got as far as binding, which is the timeout path into here.
    let _ = std::fs::remove_file(&socket);
}

/// Clear every socket in `dir` that nothing is serving.
///
/// A pass over the directory rather than over one name, because the ones worth clearing are
/// never the one this session is about to bind: the path is named after a session id that is
/// unique per magi, so nothing can be squatting on it. What accumulates is the *predecessors* —
/// a file per run, each outliving the balthasar that bound it.
///
/// Only `api@*.sock`, so the settings and the tool description sitting beside them are left
/// alone. And on the way up rather than on the way down, because the runs that leave a file are
/// exactly the ones that did not get to run anything on the way down.
fn sweep_stale(dir: &Path) {
    for path in magi_ipc::family::sockets_in(dir) {
        sweep(&path);
    }
}

/// Clear a socket at `path` that nothing is serving.
///
/// Dialled, never guessed: a live balthasar's socket looks exactly like a dead one's, so
/// unlinking on appearance would take another window's memory layer out from under it. Only a
/// refused connection is proof, and the file left by a `kill -9` is the only thing that refuses.
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

    /// A socket directory holding a live socket, a dead one, and two files that are not sockets.
    fn littered(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("magi-sweep-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let dead = dir.join("api@00000000000000000001-alpha.sock");
        {
            let _listener = std::os::unix::net::UnixListener::bind(&dead).expect("bind");
        }
        let live = dir.join("api@00000000000000000002-beta.sock");
        (dir, live, dead)
    }

    #[tokio::test]
    async fn every_socket_nobody_answers_is_cleared_not_only_this_sessions() {
        // The leak. Sweeping one path cleared a corpse of this session's own, and a session id
        // is never reused — so nothing was ever cleared and every run left a file for good.
        let (dir, live, dead) = littered("directory");
        let _listener = std::os::unix::net::UnixListener::bind(&live).expect("bind");

        sweep_stale(&dir);
        assert!(!dead.exists(), "a predecessor's socket outlived it");
        assert!(
            live.exists(),
            "another window's balthasar was taken down with it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn what_is_not_a_socket_is_left_where_it_is() {
        // The settings a coordinator wrote and the tool description sit in the same directory,
        // and a sweep that went by "everything here is stale" would take both.
        let (dir, _live, _dead) = littered("bystanders");
        let given = dir.join("given.lua");
        let tool = dir.join("balthasar.tool");
        std::fs::write(&given, "balthasar.decay = 0.5\n").expect("write");
        std::fs::write(&tool, "{}").expect("write");

        sweep_stale(&dir);
        assert!(given.exists(), "the settings went with the sockets");
        assert!(tool.exists(), "the tool description went with the sockets");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_session_takes_its_socket_with_it() {
        // "Nothing outlives the window" should be true of the file as well as the process.
        // Left behind, it was cleared by the next magi rather than by this one -- so a machine
        // at rest always had one, and the directory never quite emptied.
        let dir = std::env::temp_dir().join(format!("magi-ended-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let socket = dir.join("api@00000000000000000003-gamma.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        // A stand-in for balthasar: something that is running and holds the socket open, so
        // this is a kill and an unlink rather than a tidy exit.
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .spawn()
            .expect("a child to end");
        let id = child.id();

        ended(Ours {
            child,
            socket: socket.clone(),
        });
        drop(listener);

        assert!(!socket.exists(), "the socket outlived the session");
        assert!(
            !Path::new(&format!("/proc/{id}")).exists(),
            "the child outlived the session"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_balthasar_that_never_bound_is_still_ended() {
        // The timeout path into `stop`: the process started and never got as far as binding,
        // so there is a child to kill and no file to remove.
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .spawn()
            .expect("a child to end");
        let id = child.id();
        ended(Ours {
            child,
            socket: std::env::temp_dir().join("magi-never-bound-anything.sock"),
        });
        assert!(
            !Path::new(&format!("/proc/{id}")).exists(),
            "a missing socket left the child running"
        );
    }

    #[tokio::test]
    async fn stopping_what_was_never_started_is_quiet() {
        // The ordinary case on a machine without balthasar: `stop` runs at every exit.
        stop();
    }

    #[tokio::test]
    async fn a_directory_that_is_not_there_is_not_an_error() {
        // The first run on a machine. Nothing to sweep is the ordinary case, not a failure.
        sweep_stale(Path::new("/nonexistent/magi-sweep-nothing-here"));
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
