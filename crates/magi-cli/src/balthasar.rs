//! Starting the memory layer, and taking it down again. One balthasar per magi, named after the
//! session, not one per project: two windows would share an instance and whichever quit first would
//! take the other's store out from under it. It dies with its magi twice over — [`stop`] ends it on
//! the way out, and `--tied` asks the kernel for `PR_SET_PDEATHSIG` so a panic, a `kill -9` or an
//! OOM ends it too. Sweeping a leftover socket is not a substitute: that clears a name, and the
//! orphan holding it is a live process that still answers `verbs`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// The balthasar this process started, so it can be ended and its path cleared.
static STARTED: Mutex<Option<Ours>> = Mutex::new(None);

/// What balthasar reads a connection's agent out of — its name for it, spelled here because magi is
/// what sets it. The two programs do not link, so the coupling is a variable name and nothing else.
pub const AGENT: &str = "BALTHASAR_AGENT";

/// The id out of `project/role/id`. Told to the balthasar magi spawns, not set on this process:
/// balthasar reads it out of the peer's `/proc/<pid>/environ`, which `setenv` never touches.
pub fn agent_of(named: &str) -> Option<&str> {
    named
        .split('/')
        .nth(2)
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

/// A balthasar this magi started, and the path it was told to bind. The path is kept beside the
/// child, which is killed with a signal it cannot handle and so never unlinks its own socket.
struct Ours {
    child: Child,
    socket: PathBuf,
}

/// How long to wait for a freshly started balthasar to bind. Generous: there is no fallback,
/// and the loop below stops the moment the child exits.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

/// What came of trying to convene this session's store.
#[derive(Debug)]
pub enum Started {
    Ours(PathBuf),
    /// Somebody else already said which one to talk to. Theirs, not ours to start or to stop.
    Theirs,
    Refused(String),
}

/// Start this session's memory layer and return the socket it bound. `program` is whatever fills
/// the `memory` role — see `ROLES.md`. The reason is carried out rather than logged, because the
/// caller refuses the session and has to say the actual cause.
pub async fn start(program: &str, instance: &str, project: &Path, agent: Option<&str>) -> Started {
    // Somebody else already said which one to talk to. Theirs, not ours to start.
    if std::env::var_os("MAGI_API_SOCKET").is_some_and(|v| !v.is_empty()) {
        return Started::Theirs;
    }

    let dir = magi_ipc::family::socket_dir();
    let socket = dir.join(format!("api@{instance}.sock"));
    // The whole directory, not only the path about to be taken: a session id is unique per magi, so
    // sweeping one path only ever cleared a corpse this same session had left, which is none.
    sweep_stale(&dir);

    let mut spawning = Command::new(program);
    // In the child's initial environment, which is the only place balthasar can read it from.
    if let Some(agent) = agent {
        spawning.env(AGENT, agent);
    }
    let child = spawning
        .arg("serve")
        .arg("--instance")
        .arg(instance)
        .arg("--scope")
        .arg("project")
        // The kernel's copy of "it dies with its magi". This process names itself: an orphan has
        // already been reparented by the time it could look.
        .arg("--tied")
        .arg(std::process::id().to_string())
        .current_dir(project)
        // Piped rather than silenced: it must not reach the terminal the UI shares, but a balthasar
        // that refuses to start needs its last words, which are all a person has to go on.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(why) => {
            return Started::Refused(format!("`{program} serve` could not be started: {why}"));
        }
    };
    if let Ok(mut held) = STARTED.lock() {
        *held = Some(Ours {
            child,
            socket: socket.clone(),
        });
    }

    // Polled rather than assumed: a socket appears when balthasar binds it. The child is watched as
    // well, so an install that exits at once is not reported twenty seconds later as a timeout.
    let deadline = std::time::Instant::now() + PATIENCE;
    let mut bound = false;
    while std::time::Instant::now() < deadline {
        match reached(&socket).await {
            Reached::Answering => return Started::Ours(socket),
            Reached::Bound => bound = true,
            Reached::Nothing => {}
        }
        if let Some(status) = exited() {
            let said = last_words();
            stop();
            return Started::Refused(match said.is_empty() {
                true => format!(
                    "`{program} serve` exited ({status}) without binding {}",
                    socket.display()
                ),
                false => format!("`{program} serve` exited ({status}): {said}"),
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    // Bound and still busy opening its store. The session may have it: a write is on a clock of its
    // own, long enough to outlast the rest of that, where refusing here loses the session outright.
    if bound {
        return Started::Ours(socket);
    }
    stop();
    Started::Refused(format!(
        "{program} did not bind {} within {PATIENCE:?}",
        socket.display()
    ))
}

/// How far a poll got: balthasar opens its store on the first call, so it answers after it binds.
enum Reached {
    Nothing,
    Bound,
    Answering,
}

/// Dial, and ask for something every balthasar answers.
async fn reached(path: &Path) -> Reached {
    let Ok(mut open) = magi_ipc::family::Family::dial(path).await else {
        return Reached::Nothing;
    };
    match open.call("verbs", Vec::new()).await {
        Ok(_) => Reached::Answering,
        Err(_) => Reached::Bound,
    }
}

/// How the balthasar this process started ended, if it has. Reaped through the handle, not by
/// pid, so nothing races a reaper.
fn exited() -> Option<std::process::ExitStatus> {
    let mut held = STARTED.lock().ok()?;
    held.as_mut()?.child.try_wait().ok().flatten()
}

/// What a balthasar that would not start said on its way out: the last line only, read after the
/// child has exited, so the pipe is closed and this cannot block.
fn last_words() -> String {
    use std::io::Read;
    let mut said = String::new();
    if let Ok(mut held) = STARTED.lock()
        && let Some(ours) = held.as_mut()
        && let Some(pipe) = ours.child.stderr.as_mut()
    {
        let _ = pipe.read_to_string(&mut said);
    }
    said.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// End the balthasar this process started and clear its path, reaping before the unlink: after
/// [`std::process::Child::wait`] the name cannot still be answering.
pub fn stop() {
    let Ok(mut held) = STARTED.lock() else {
        return;
    };
    let Some(ours) = held.take() else {
        return;
    };
    ended(ours);
}

/// Split from [`stop`] so the order can be tested without the process-wide static.
fn ended(Ours { mut child, socket }: Ours) {
    let _ = child.kill();
    let _ = child.wait();
    // Absent when it never got as far as binding, which is the timeout path into here.
    let _ = std::fs::remove_file(&socket);
}

/// Clear every socket in `dir` that nothing is serving — a pass over the directory, because what
/// accumulates is predecessors. Only `api@*.sock`, so settings and tool descriptions are spared.
fn sweep_stale(dir: &Path) {
    for path in magi_ipc::family::sockets_in(dir) {
        sweep(&path);
    }
}

/// Clear a socket at `path` that nothing is serving. Asked rather than merely dialled: the kernel
/// accepts on behalf of a listener whose owner has stopped reading, so one `verbs` call settles it.
fn sweep(path: &Path) {
    if !path.exists() {
        return;
    }
    let answered = magi_ipc::family::blocking::Family::dial(path)
        .is_ok_and(|mut open| open.call("verbs", Vec::new()).is_ok());
    if !answered {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    /// Held by every test that binds a socket or starts a process: `fork` copies the descriptor
    /// table, so a spawn during another thread's bind keeps that socket open until it `exec`s.
    static ALONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take [`ALONE`], ignoring a poisoning left by some other test's failure.
    fn alone() -> std::sync::MutexGuard<'static, ()> {
        ALONE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A socket something is actually serving on: it accepts, and it answers. A bare `UnixListener`
    /// cannot stand in — an inherited descriptor accepts but cannot answer.
    fn serving(path: &std::path::Path) -> std::thread::JoinHandle<()> {
        use std::io::{Read, Write};

        let listener = std::os::unix::net::UnixListener::bind(path).expect("bind");
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut head = [0_u8; 4];
            if stream.read_exact(&mut head).is_err() {
                return;
            }
            let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
            if stream.read_exact(&mut body).is_err() {
                return;
            }
            let reply = br#"{"ok":true,"result":[]}"#;
            let mut framed = (reply.len() as u32).to_be_bytes().to_vec();
            framed.extend_from_slice(reply);
            let _ = stream.write_all(&framed);
        })
    }

    #[tokio::test]
    async fn a_named_socket_is_not_taken_from_a_live_balthasar() {
        let _alone = alone();
        // The sweep must ask rather than stat, and rather than merely dial.
        let dir = Scratch::new("magi-sweep", "one");
        let path = dir.join("api@live.sock");
        let served = serving(&path);

        sweep(&path);
        assert!(path.exists(), "a socket something is serving must survive");
        let _ = served.join();
    }

    #[tokio::test]
    async fn a_socket_nothing_answers_is_cleared() {
        let _alone = alone();
        let dir = Scratch::new("magi-sweep-dead", "one");
        let path = dir.join("api@dead.sock");
        {
            let _listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        }

        sweep(&path);
        assert!(!path.exists(), "a socket nothing answers must be cleared");
    }

    /// A socket directory holding a live socket, a dead one, and two files that are not sockets.
    fn littered(name: &str) -> (Scratch, PathBuf, PathBuf) {
        let dir = Scratch::new("magi-sweep", name);
        let dead = dir.join("api@00000000000000000001-alpha.sock");
        {
            let _listener = std::os::unix::net::UnixListener::bind(&dead).expect("bind");
        }
        let live = dir.join("api@00000000000000000002-beta.sock");
        (dir, live, dead)
    }

    #[tokio::test]
    async fn every_socket_nobody_answers_is_cleared_not_only_this_sessions() {
        let _alone = alone();
        // A session id is never reused, so sweeping one path only ever cleared a corpse of its own.
        let (dir, live, dead) = littered("directory");
        let served = serving(&live);

        sweep_stale(&dir);
        assert!(!dead.exists(), "a predecessor's socket outlived it");
        assert!(
            live.exists(),
            "another window's balthasar was taken down with it"
        );
        let _ = served.join();
    }

    #[tokio::test]
    async fn what_is_not_a_socket_is_left_where_it_is() {
        let _alone = alone();
        // The settings a coordinator wrote and the tool description sit in the same directory.
        let (dir, _live, _dead) = littered("bystanders");
        let given = dir.join("given.lua");
        let tool = dir.join("balthasar.tool");
        std::fs::write(&given, "balthasar.decay = 0.5\n").expect("write");
        std::fs::write(&tool, "{}").expect("write");

        sweep_stale(&dir);
        assert!(given.exists(), "the settings went with the sockets");
        assert!(tool.exists(), "the tool description went with the sockets");
    }

    #[tokio::test]
    async fn a_session_takes_its_socket_with_it() {
        let _alone = alone();
        // Left behind, it was cleared by the next magi rather than by this one.
        let dir = Scratch::new("magi-ended", "one");
        let socket = dir.join("api@00000000000000000003-gamma.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        // A stand-in for balthasar: running and holding the socket open, so this is a kill.
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
    }

    #[tokio::test]
    async fn a_balthasar_that_never_bound_is_still_ended() {
        let _alone = alone();
        // The timeout path into `stop`: a child to kill and no file to remove.
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
        // The first run on a machine: nothing to sweep is ordinary, not a failure.
        sweep_stale(Path::new("/nonexistent/magi-sweep-nothing-here"));
    }

    #[test]
    fn the_agent_is_the_last_part_of_the_name_melchior_gave() {
        assert_eq!(agent_of("magi/main/alpha-rho"), Some("alpha-rho"));
        assert_eq!(agent_of("magi/reviewer/zeta-pi"), Some("zeta-pi"));
    }

    #[test]
    fn a_session_with_no_melchior_is_named_nothing_rather_than_a_guess() {
        // An invented agent name files somebody's memory under a name that does not exist.
        for named in ["", "magi", "magi/main", "magi/main/", "magi/main/   "] {
            assert_eq!(agent_of(named), None, "{named:?} was named as something");
        }
    }

    #[tokio::test]
    async fn a_socket_somebody_else_named_is_not_ours_to_start() {
        // Set for the length of this test only, and read before anything is spawned.
        let saved = std::env::var_os("MAGI_API_SOCKET");
        assert!(
            saved.is_none()
                || matches!(
                    start("balthasar", "x", Path::new("/tmp"), None).await,
                    Started::Theirs
                ),
            "an explicit socket means somebody else's balthasar — not ours to start, and not a \
             refusal either"
        );
    }
}
