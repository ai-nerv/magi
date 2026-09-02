//! The session, running inside the process that shows it.
//!
//! There is no daemon. There was: `axon` spawned `axon host` as a background child that owned
//! the journal and the socket, and a UI quitting was a *detach* — the child stayed up. Two
//! things came of that, and both were wrong.
//!
//! The first was invisible until somebody opened two windows. The daemon's socket was named
//! after the working directory, so the second `axon` in a project found the first one's daemon
//! already answering and attached to it. Two windows, one session, one transcript: whatever
//! either of them typed appeared in both. Every instance name axon had just learned to write
//! was a fiction over a single conversation.
//!
//! The second was the pile. Nothing ever ended a daemon, so a week of work left a process per
//! project holding a socket, a model and the environment of whichever shell happened to start
//! it — and `axon stop` existed only to clean up after a design that leaked.
//!
//! So the host is a task here, in the process that draws the screen. It binds before the first
//! frame and it goes when the process goes, because it *is* the process. One `axon` is one
//! instance: one name, one journal, one conversation, and nothing left behind.
//!
//! # Why there is still a socket
//!
//! The UI and the session speak the same framed protocol they always did, over a socket this
//! process binds and unlinks. Kept rather than replaced with a channel, because it is what
//! `axon fake-host` answers — the replay host is how the UI is developed without a model, and
//! a UI that could only talk to something in its own address space could not be pointed at it.

use anyhow::{Context, Result};
use std::path::Path;

/// Open this session and start serving it, without waiting for it to finish.
///
/// Bound before returning, so the UI's first dial cannot race the bind. Everything after that
/// is a task: the caller goes on to draw.
///
/// `resume` continues this directory's most recent journal instead of starting one.
pub async fn start(
    socket: &Path,
    sessions: Option<&Path>,
    resume: bool,
    cwd: &Path,
    loaded: Option<&crate::config::Loaded>,
    environ: &std::collections::BTreeMap<String, String>,
    whose: &str,
) -> Result<()> {
    let dir = sessions.map_or_else(axon_host::paths::sessions_dir, Path::to_path_buf);
    let cwd = cwd.display().to_string();
    let session = match resume
        .then(|| axon_host::paths::latest_for(&dir, &cwd))
        .flatten()
    {
        Some(path) => axon_host::session::Session::open(
            &path,
            axon_proto::SessionId::new(axon_host::paths::session_id(unix_seconds(), whose)),
            &cwd,
            unix_seconds(),
        )?,
        None => axon_host::open_session(&dir, &cwd, unix_seconds(), whose)?,
    };
    // A stale socket cannot be a running session any more — nothing outlives its process — so
    // one found here was left by a crash and is cleared rather than treated as somebody's.
    if let Some(parent) = socket.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = axon_ipc::bind(socket)
        .await
        .with_context(|| format!("binding {}", socket.display()))?;

    let backend = loaded.and_then(crate::config::backend);
    let mut catalog =
        loaded.map_or_else(axon_host::catalog::Catalog::empty, crate::config::catalog);
    // What every process this session starts inherits, including the tool peer that reaches
    // other instances. It is the configured environment plus this session's own name, and the
    // name is the half a separate process cannot work out for itself.
    catalog.environ = environ.clone();
    tokio::spawn(async move {
        let _ = axon_host::serve_catalog(listener, session, backend, catalog).await;
    });
    Ok(())
}

/// Take the socket back down.
///
/// A path nothing answers is indistinguishable from a session that is merely busy, and the next
/// `axon` in this project would meet it as a name already taken.
pub fn done(socket: &Path) {
    let _ = std::fs::remove_file(socket);
}

/// Seconds since the epoch, for naming a session.
///
/// A session id is a sortable timestamp, which is what makes "the most recent session" a
/// directory listing rather than an index to maintain.
fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
