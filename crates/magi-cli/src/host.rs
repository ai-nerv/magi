//! The session, running inside the process that shows it.
//!
//! There is no daemon. There was: `magi` spawned `magi host` as a background child that owned
//! the journal and the socket, and a UI quitting was a *detach* — the child stayed up. Two
//! things came of that, and both were wrong.
//!
//! The first was invisible until somebody opened two windows. The daemon's socket was named
//! after the working directory, so the second `magi` in a project found the first one's daemon
//! already answering and attached to it. Two windows, one session, one transcript: whatever
//! either of them typed appeared in both. Every instance name magi had just learned to write
//! was a fiction over a single conversation.
//!
//! The second was the pile. Nothing ever ended a daemon, so a week of work left a process per
//! project holding a socket, a model and the environment of whichever shell happened to start
//! it — and `magi stop` existed only to clean up after a design that leaked.
//!
//! So the host is a task here, in the process that draws the screen. It binds before the first
//! frame and it goes when the process goes, because it *is* the process. One `magi` is one
//! instance: one name, one journal, one conversation, and nothing left behind.
//!
//! # Why there is still a socket
//!
//! The UI and the session speak the same framed protocol they always did, over a socket this
//! process binds and unlinks. Kept rather than replaced with a channel, because it is what
//! `magi fake-host` answers — the replay host is how the UI is developed without a model, and
//! a UI that could only talk to something in its own address space could not be pointed at it.

use anyhow::{Context, Result};
use std::os::unix::fs::FileTypeExt;
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
    key: &str,
) -> Result<()> {
    let dir = sessions.map_or_else(magi_host::paths::sessions_dir, Path::to_path_buf);
    let cwd = cwd.display().to_string();
    let id = magi_proto::SessionId::new(magi_host::paths::session_id(unix_seconds(), key));
    // With balthasar running there is no journal on disk at all: it is the store, and a second
    // copy is a copy that goes stale. Without it, the file is the store exactly as before.
    let mut carried = match magi_ipc::family::Family::find(None).await {
        Ok(family) => {
            let mut scribe = magi_host::scribe::Scribe::over(family, &id);
            Some(match resume.then(|| resumable(&mut scribe)) {
                Some(fut) => fut.await,
                None => Vec::new(),
            })
        }
        Err(_) => None,
    };
    let session = match carried.take() {
        Some(entries) => magi_host::session::Session::recorded(id, entries),
        None => match resume.then(|| free(&dir, &cwd, socket.parent())).flatten() {
            Some(path) => magi_host::session::Session::open(&path, id, &cwd, unix_seconds())?,
            None => magi_host::open_session(&dir, &cwd, unix_seconds(), key)?,
        },
    };
    // A stale socket cannot be a running session any more — nothing outlives its process — so
    // one found here was left by a crash and is cleared rather than treated as somebody's.
    if let Some(parent) = socket.parent() {
        tokio::fs::create_dir_all(parent).await?;
        sweep(parent);
    }
    let listener = magi_ipc::bind(socket)
        .await
        .with_context(|| format!("binding {}", socket.display()))?;

    let mut backend = loaded.and_then(crate::config::backend);
    let mut catalog =
        loaded.map_or_else(magi_host::catalog::Catalog::empty, crate::config::catalog);
    stamp(&mut backend, &mut catalog, environ);
    tokio::spawn(async move {
        let _ = magi_host::serve_catalog(listener, session, backend, catalog).await;
    });
    Ok(())
}

/// Take the socket back down.
///
/// A path nothing answers is indistinguishable from a session that is merely busy, and the next
/// `magi` in this project would meet it as a name already taken.
pub fn done(socket: &Path) {
    let _ = std::fs::remove_file(socket);
    // And the directory, if this was the last session in the project. `remove_dir` refuses one
    // that still holds something, which is the whole test: whoever leaves last does it, and a
    // session binding at the same moment is not raced.
    if let Some(parent) = socket.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

/// Clear out sockets in `dir` that nothing is serving.
///
/// Run at startup rather than only at exit, because the sessions that need clearing are the ones
/// that never reached their exit path: a crash, a kill, or a build that named its socket
/// differently. Ten of those had collected in one project here, and nothing would ever have
/// removed them — the directory is how a session is found, so litter in it is not cosmetic.
///
/// Dialled, never guessed. Unlinking a path because it looks stale would take a live session's
/// socket out from under it, and both would then believe they were reachable.
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_socket())
            && std::os::unix::net::UnixStream::connect(&path).is_err()
        {
            let _ = std::fs::remove_file(&path);
        }
    }
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

/// The newest journal for `cwd` that nothing is still writing to.
///
/// `--resume` used to mean "the newest one here", full stop, and that was fine while a directory
/// had one session. It does not any more: two `magi -r` in one project both took the newest,
/// both opened it, and appended into one file in whatever order they happened to write — a
/// transcript neither of them said.
///
/// A journal is named after the session that made it and a session id ends in that session's
/// key, so "is anybody still writing this" is a question `sockets` answers: if something is
/// listening on the socket that key names, the journal is taken. `None` means every one of them
/// is, which is a fresh session rather than a refusal — somebody asking to resume wants to start
/// working.
///
/// Dialled rather than looked for. A path is left behind by a crash, and a journal nobody could
/// ever resume again because the session that wrote it died badly is worse than one opened twice.
fn free(dir: &Path, cwd: &str, sockets: Option<&Path>) -> Option<std::path::PathBuf> {
    magi_host::paths::summaries(dir, cwd)
        .into_iter()
        .find(|session| {
            let Some(whose) = session.id.split_once('-').map(|(_, key)| key) else {
                // A journal from before session ids carried a key. Nothing can be checked, and
                // the old behaviour is the right one for it.
                return true;
            };
            !answers(sockets, whose)
        })
        .map(|session| session.path)
}

/// Whether a session with this key is still up.
///
/// Connecting is the whole test: a socket with nothing behind it refuses, and one still being
/// served accepts. Nothing is sent — the question is whether anybody is there, and asking it
/// twice would be a protocol.
fn answers(sockets: Option<&Path>, key: &str) -> bool {
    sockets.is_some_and(|dir| {
        std::os::unix::net::UnixStream::connect(crate::session::socket_in(dir, key)).is_ok()
    })
}

/// Put this session's environment where every process it starts will pick it up.
///
/// **Both, and the reason is not symmetry.** Tools are built from the *backend*, so stamping the
/// catalog alone left the `agent` peer with no name: `mine()` answered `None` and every verb
/// refused with "this process was not started by an magi session" — a session reachable by name
/// that could reach nobody. And the catalog is what a `/model` switch rebuilds a backend from,
/// so stamping the backend alone would have worked right up until somebody changed model.
fn stamp(
    backend: &mut Option<magi_host::turn::Backend>,
    catalog: &mut magi_host::catalog::Catalog,
    environ: &std::collections::BTreeMap<String, String>,
) {
    catalog.environ = environ.clone();
    if let Some(backend) = backend.as_mut() {
        backend.environ = environ.clone();
    }
}

/// A tool peer can find out which session it belongs to.
#[cfg(test)]
mod tests {
    use super::*;

    fn environ() -> std::collections::BTreeMap<String, String> {
        [
            ("MAGI_MELCHIOR_PROJECT".to_owned(), "magi".to_owned()),
            ("MAGI_MELCHIOR_ROLE".to_owned(), "main".to_owned()),
            ("MAGI_MELCHIOR_ID".to_owned(), "delta-rho".to_owned()),
        ]
        .into_iter()
        .collect()
    }

    /// A catalog holding one model that needs no credential, so it yields a real backend.
    fn catalog() -> magi_host::catalog::Catalog {
        let mut catalog = magi_host::catalog::Catalog::empty();
        catalog.providers = vec![magi_provider::provider::Provider {
            id: "fake".into(),
            name: "Fake".into(),
            base_url: Some("http://127.0.0.1:1/v1".into()),
            api: magi_provider::model::Api::OpenAiCompletions,
            auth: magi_provider::provider::Auth::None,
            compat: None,
            models: vec![magi_provider::model::Model {
                id: "m".into(),
                provider: "fake".into(),
                name: "M".into(),
                api: magi_provider::model::Api::OpenAiCompletions,
                reasoning: false,
                input: magi_provider::model::default_input(),
                context_window: 1000,
                max_tokens: 100,
                cost: magi_model::Cost::default(),
                thinking: std::collections::BTreeMap::new(),
                compat: None,
            }],
            discover: false,
        }];
        catalog
    }

    #[test]
    fn the_session_s_name_reaches_the_tools_it_starts() {
        // The bug this is here for. A tool peer is spawned from the *backend*'s environment, so
        // a name put only on the catalog never reached it, and the `agent` tool answered every
        // verb with "this process was not started by an magi session" — a session reachable by
        // name that could reach nobody.
        let mut catalog = catalog();
        let mut backend = catalog.backend("fake/m");
        assert!(backend.is_some(), "the fixture yields a backend");
        stamp(&mut backend, &mut catalog, &environ());

        let started = backend.expect("a backend").environ;
        assert_eq!(
            started.get("MAGI_MELCHIOR_ID").map(String::as_str),
            Some("delta-rho")
        );
        assert_eq!(
            started.get("MAGI_MELCHIOR_PROJECT").map(String::as_str),
            Some("magi")
        );
        assert_eq!(
            started.get("MAGI_MELCHIOR_ROLE").map(String::as_str),
            Some("main")
        );
    }

    #[test]
    fn and_survives_a_change_of_model() {
        // `/model` builds a fresh backend from the catalog, so a name stamped only on the
        // backend would have been lost the moment somebody switched.
        let mut catalog = catalog();
        let mut backend = catalog.backend("fake/m");
        stamp(&mut backend, &mut catalog, &environ());

        let after = catalog.backend("fake/m").expect("still there").environ;
        assert_eq!(
            after.get("MAGI_MELCHIOR_ID").map(String::as_str),
            Some("delta-rho"),
            "a switch lost the session's name"
        );
    }

    /// A journal in `dir` for `cwd`, named after the session that made it.
    fn journal(dir: &Path, id: &str, cwd: &str) {
        let path = dir.join(format!("{id}.jsonl"));
        magi_journal::Journal::open(&path, magi_proto::SessionId::new(id.to_owned()), cwd, 1)
            .expect("journal");
    }

    #[test]
    fn resuming_takes_the_newest_journal_nobody_is_writing_to() {
        // Two `magi -r` in one project both used to take the newest, both open it, and append
        // into one file in whatever order they happened to write.
        let dir = std::env::temp_dir().join(format!("magi-free-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        journal(&dir, "00000000000000000001-alpha-rho", "/work");
        journal(&dir, "00000000000000000002-beta-nu", "/work");

        // Nothing is listening in either name, so the newest wins as it always did.
        let found = free(&dir, "/work", Some(&dir)).expect("a journal");
        assert!(found.to_string_lossy().contains("beta-nu"), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_journal_from_before_names_is_still_resumable() {
        // Written when a session id was a bare timestamp. Nothing can be checked about it, and
        // refusing to resume it would lose somebody their history over a naming change.
        let dir = std::env::temp_dir().join(format!("magi-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        journal(&dir, "00000000000000000007", "/work");
        assert!(free(&dir, "/work", Some(&dir)).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_to_resume_is_a_fresh_session_rather_than_a_refusal() {
        // Somebody asking to resume wants to start working.
        let dir = std::env::temp_dir().join(format!("magi-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(free(&dir, "/work", Some(&dir)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_with_no_model_still_names_itself() {
        // Every `agent` verb works without one, and a session that cannot answer a prompt can
        // still be asked what it is doing.
        let mut catalog = magi_host::catalog::Catalog::empty();
        let mut nothing = None;
        stamp(&mut nothing, &mut catalog, &environ());
        assert!(nothing.is_none());
        assert_eq!(
            catalog.environ.get("MAGI_MELCHIOR_ID").map(String::as_str),
            Some("delta-rho")
        );
    }
}

/// Nothing a session leaves behind outlives it.
#[cfg(test)]
mod leftovers {
    use super::*;

    #[test]
    fn a_socket_nothing_answers_is_cleared_and_a_live_one_is_not() {
        // Ten of these had collected in one project, from crashes and from a build that named
        // its socket differently, and nothing would ever have removed them. The directory is how
        // a session is found, so litter in it is not cosmetic.
        let dir = std::env::temp_dir().join(format!("magi-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let live = std::os::unix::net::UnixListener::bind(dir.join("alive.host")).expect("bind");
        // A socket with nothing behind it: bound, then the listener dropped.
        let dead = dir.join("dead.host");
        drop(std::os::unix::net::UnixListener::bind(&dead).expect("bind"));
        // And something that is not a socket at all, which must be left alone.
        std::fs::write(dir.join("keep.me"), b"not mine").expect("write");

        sweep(&dir);

        assert!(dir.join("alive.host").exists(), "a live session was swept");
        assert!(!dead.exists(), "a dead socket was left behind");
        assert!(
            dir.join("keep.me").exists(),
            "something not a socket was removed"
        );
        drop(live);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_session_out_takes_the_directory_with_it() {
        let dir = std::env::temp_dir().join(format!("magi-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let socket = dir.join("one.host");
        std::fs::write(&socket, b"").expect("write");

        done(&socket);
        assert!(!dir.exists(), "an empty project directory was left behind");
    }

    #[test]
    fn a_directory_somebody_else_is_still_in_stays() {
        // The test is `remove_dir` refusing a directory that holds something, which is what
        // makes this safe without a listing and without racing a session that is binding.
        let dir = std::env::temp_dir().join(format!("magi-busy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("mine.host"), b"").expect("write");
        std::fs::write(dir.join("theirs.host"), b"").expect("write");

        done(&dir.join("mine.host"));
        assert!(
            dir.exists(),
            "a directory with a session still in it was removed"
        );
        assert!(dir.join("theirs.host").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The newest run balthasar holds for this project, for `--resume`.
///
/// Empty when there is nothing to carry on from, which is a fresh session rather than a
/// refusal: somebody asking to resume wants to start working.
async fn resumable(scribe: &mut magi_host::scribe::Scribe) -> Vec<magi_proto::Entry> {
    let Ok(rows) = scribe.sessions().await else {
        return Vec::new();
    };
    let newest = rows
        .iter()
        .flat_map(|value| match value.as_array() {
            Some(list) => list.clone(),
            None => vec![value.clone()],
        })
        .filter_map(|row| {
            row.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .next();
    match newest {
        Some(id) => scribe.replay_of(&id).await.unwrap_or_default(),
        None => Vec::new(),
    }
}
