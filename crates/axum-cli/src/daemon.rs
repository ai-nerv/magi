//! Making sure a daemon is listening before anything tries to talk to one.
//!
//! Every entry point goes through here, including `-p`. A one-shot that ran the turn in its
//! own process would be a second implementation of the loop, and the two would drift; it would
//! also leave nothing behind, so the answer it printed could not be resumed or read back. The
//! daemon is the session, so a one-shot borrows one rather than avoiding it.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// How long to wait for a daemon we just started to bind its socket.
///
/// Generous because the first start loads the Lua config and builds the VM; the wait ends on
/// the first successful connection, so the limit only matters when something is wrong.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

/// How often to retry the connection while waiting.
const POLL: Duration = Duration::from_millis(25);

/// The longest a Unix socket path may be.
///
/// `sockaddr_un.sun_path` is 108 bytes on Linux, NUL included. Nothing checked, so a long path
/// produced a daemon that announced the session it was about to serve, failed to bind with
/// `path must be shorter than SUN_LEN`, and exited -- while the UI waited twenty seconds and
/// then blamed it for not answering.
const SUN_LEN: usize = 108;

/// Connect to the daemon for `socket`, starting one if nothing answers.
///
/// Answers **whether this call started it**. The UI stops a daemon it started when it exits, so
/// it has to know which of the two happened: attaching to somebody else's daemon and then
/// killing it on the way out would end their session.
///
/// A socket that exists but refuses is a daemon that died; `bind` clears the stale file, so
/// starting one is the right response to both cases and they need no distinguishing here.
pub async fn ensure(socket: &Path, sessions: Option<&Path>, resume: bool) -> Result<bool> {
    if axum_ipc::connect(socket).await.is_ok() {
        return Ok(false);
    }
    // Before anything is spawned, because the failure is certain and the daemon's own report
    // of it lands on a stderr nobody is reading.
    let length = socket.as_os_str().len();
    if length >= SUN_LEN {
        bail!(
            "the socket path is {length} bytes and a Unix socket may be at most {}: {}",
            SUN_LEN - 1,
            socket.display()
        );
    }
    let mut child = spawn(socket, sessions, resume)?;

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if axum_ipc::connect(socket).await.is_ok() {
            return Ok(true);
        }
        // A daemon that has already exited is not going to answer, and waiting the rest of
        // the twenty seconds to say so turns a one-line error into a hang.
        if let Ok(Some(status)) = child.try_wait() {
            let _ = std::fs::remove_file(pid_path(socket));
            bail!("the daemon exited ({status}){}", said(socket));
        }
        tokio::time::sleep(POLL).await;
    }
    bail!(
        "no daemon answered on {} within {}s{}",
        socket.display(),
        STARTUP_TIMEOUT.as_secs(),
        said(socket)
    )
}

/// What the daemon wrote on its way out, if anything.
///
/// Its stderr goes to a file rather than to the terminal, which the UI is drawing on -- so on
/// a failed start there is something to quote instead of a guess.
fn said(socket: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(log_path(socket)) else {
        return String::new();
    };
    let last = text.lines().rev().find(|l| !l.trim().is_empty());
    last.map_or_else(String::new, |line| format!(": {line}"))
}

/// Where the daemon's own output is kept, beside its socket.
#[must_use]
pub fn log_path(socket: &Path) -> PathBuf {
    socket.with_extension("log")
}

/// Start `axum host` in the background.
///
/// The child is left running on purpose: it owns the session, and a UI that quits should be a
/// detach rather than an end of the conversation. Its output goes nowhere because both callers
/// own the terminal — a UI is drawing on it and a `-p` run is writing the answer to it.
fn spawn(socket: &Path, sessions: Option<&Path>, resume: bool) -> Result<std::process::Child> {
    let exe = std::env::current_exe().context("finding the axum binary")?;
    let mut command = std::process::Command::new(exe);
    command.arg("--socket").arg(socket).arg("host");
    if let Some(dir) = sessions {
        command.arg("--sessions").arg(dir);
    }
    if resume {
        command.arg("--resume");
    }
    // The directory is created before the spawn now, because the log file lands in it too and
    // a daemon that cannot open its log is a daemon whose failure is unreadable.
    if let Some(parent) = socket.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // To a file rather than to the terminal, which a UI is drawing on and a `-p` run is
    // writing the answer to; and to a file rather than a pipe, because a pipe nobody drains
    // eventually blocks the daemon on its own logging.
    let log = std::fs::File::create(log_path(socket)).map_or_else(|_| Stdio::null(), Stdio::from);
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .with_context(|| format!("starting a daemon on {}", socket.display()))?;
    // Best effort: a daemon that runs without its pid recorded is still a working daemon, and
    // failing the run over a file nothing has read yet would be the worse outcome.
    let _ = std::fs::write(pid_path(socket), child.id().to_string());
    Ok(child)
}

/// Where the daemon's process id is recorded, beside its socket.
///
/// A socket proves a daemon was started once; it does not say which process is serving it, and
/// a stale one says nothing at all. The pid is what makes "stop the daemon for this directory"
/// answerable without searching the process table for a command line that looks about right --
/// a search that matches more than it should as soon as two paths share a prefix.
#[must_use]
pub fn pid_path(socket: &Path) -> PathBuf {
    socket.with_extension("pid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("axum-daemon-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{name}.sock"))
    }

    #[tokio::test]
    async fn a_listening_daemon_is_not_replaced() {
        let path = socket("live");
        let _listener = axum_ipc::bind(&path).await.expect("bind");
        // Returns from the first connect: a spawn here would race a second daemon onto a
        // socket that is already bound, and one of the two would serve nobody.
        tokio::time::timeout(Duration::from_secs(1), ensure(&path, None, false))
            .await
            .expect("no spawn was attempted")
            .expect("connected");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_pid_file_sits_beside_the_socket() {
        let path = socket("named");
        assert_eq!(pid_path(&path), path.with_extension("pid"));
        assert_eq!(pid_path(&path).parent(), path.parent());
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[tokio::test]
    async fn a_socket_path_too_long_is_refused_before_anything_is_started() {
        // The daemon announced the session it was about to serve, failed to bind, and exited.
        // The UI waited twenty seconds and then blamed it for not answering.
        let long = std::env::temp_dir()
            .join("x".repeat(SUN_LEN))
            .join("a.sock");
        let error = ensure(&long, None, false)
            .await
            .expect_err("a path that cannot bind is not a wait");
        let text = error.to_string();
        assert!(text.contains("at most"), "{text}");
        assert!(text.contains("107"), "it names the limit: {text}");
    }

    #[tokio::test]
    async fn a_refusal_is_immediate_rather_than_a_twenty_second_wait() {
        let long = std::env::temp_dir()
            .join("y".repeat(SUN_LEN))
            .join("a.sock");
        let started = Instant::now();
        let _ = ensure(&long, None, false).await;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it did not wait"
        );
    }

    #[test]
    fn the_log_sits_beside_the_socket() {
        let path = std::env::temp_dir().join("axum-log-test.sock");
        assert_eq!(log_path(&path), path.with_extension("log"));
        assert_ne!(log_path(&path), pid_path(&path));
    }

    #[test]
    fn what_the_daemon_said_is_its_last_real_line() {
        let dir = std::env::temp_dir().join(format!("axum-said-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let socket = dir.join("a.sock");
        std::fs::write(log_path(&socket), "starting\nError: it broke\n\n").expect("write");
        assert_eq!(said(&socket), ": Error: it broke");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_log_is_no_quote_rather_than_an_empty_one() {
        let socket = std::env::temp_dir().join("axum-no-log-at-all.sock");
        let _ = std::fs::remove_file(log_path(&socket));
        assert_eq!(said(&socket), "");
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[tokio::test]
    async fn attaching_to_a_live_daemon_claims_nothing() {
        // The UI stops a daemon it started. Attaching to somebody else's and then killing it on
        // the way out would end their session, so `ensure` has to say which of the two happened.
        let dir = std::env::temp_dir().join(format!("axum-own-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("live.sock");
        let _listener = axum_ipc::bind(&path).await.expect("bind");

        let spawned = tokio::time::timeout(Duration::from_secs(1), ensure(&path, None, false))
            .await
            .expect("no spawn was attempted")
            .expect("connected");
        assert!(
            !spawned,
            "this call did not start it, so it does not own it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_refusal_claims_nothing_either() {
        // A path that cannot bind starts nothing, so there is nothing to stop afterwards.
        let long = std::env::temp_dir()
            .join("z".repeat(SUN_LEN))
            .join("a.sock");
        assert!(ensure(&long, None, false).await.is_err());
    }
}
