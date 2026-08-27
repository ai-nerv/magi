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

/// Connect to the daemon for `socket`, starting one if nothing answers.
///
/// A socket that exists but refuses is a daemon that died; `bind` clears the stale file, so
/// starting one is the right response to both cases and they need no distinguishing here.
pub async fn ensure(socket: &Path, sessions: Option<&Path>, resume: bool) -> Result<()> {
    if axum_ipc::connect(socket).await.is_ok() {
        return Ok(());
    }
    spawn(socket, sessions, resume)?;

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if axum_ipc::connect(socket).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(POLL).await;
    }
    bail!(
        "no daemon answered on {} within {}s",
        socket.display(),
        STARTUP_TIMEOUT.as_secs()
    )
}

/// Start `axum host` in the background.
///
/// The child is left running on purpose: it owns the session, and a UI that quits should be a
/// detach rather than an end of the conversation. Its output goes nowhere because both callers
/// own the terminal — a UI is drawing on it and a `-p` run is writing the answer to it.
fn spawn(socket: &Path, sessions: Option<&Path>, resume: bool) -> Result<()> {
    let exe = std::env::current_exe().context("finding the axum binary")?;
    let mut command = std::process::Command::new(exe);
    command.arg("--socket").arg(socket).arg("host");
    if let Some(dir) = sessions {
        command.arg("--sessions").arg(dir);
    }
    if resume {
        command.arg("--resume");
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting a daemon on {}", socket.display()))?;
    // The directory is created here rather than left to the daemon: it makes it on its way to
    // binding, which is after this write, so without this the pid file lands nowhere on the
    // very first run in a directory and the daemon becomes unfindable.
    if let Some(parent) = socket.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Best effort otherwise: a daemon that runs without its pid recorded is still a working
    // daemon, and failing the run over a file nothing has read yet would be the worse outcome.
    let _ = std::fs::write(pid_path(socket), child.id().to_string());
    Ok(())
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
