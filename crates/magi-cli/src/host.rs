//! The session, running inside the process that shows it. There is no daemon: the host is a task
//! here, and it goes when the process goes, so one `magi` is one instance — one name, one journal,
//! one conversation. A daemon named its socket after the working directory, so a second `magi` in a
//! project attached to the first one's session. The socket is kept rather than replaced with a
//! channel because it is what `magi fake-host` answers.

use anyhow::{Context, Result};
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

/// The three names a session opens under — one parameter rather than three, because two of them are
/// `Option<&str>` and a call site that swapped them would compile.
pub struct Named<'a> {
    /// This process's own, for its host socket and the balthasar it convenes.
    pub key: &'a str,
    pub run: Option<&'a str>,
    pub agent: Option<&'a str>,
}

/// Open this session and start serving it without waiting for it to finish. Bound before returning,
/// so the UI's first dial cannot race the bind. `resume` is balthasar's — see [`resumable`].
pub async fn start(
    socket: &Path,
    resume: bool,
    cwd: &Path,
    loaded: Option<&crate::config::Loaded>,
    environ: &std::collections::BTreeMap<String, String>,
    named: Named<'_>,
) -> Result<()> {
    let Named { key, run, agent } = named;
    let cwd = cwd.display().to_string();
    let id = magi_proto::SessionId::new(recorded_as(run, key));
    // Told before started: a sibling reads what a coordinator said as it comes up.
    if let Some(loaded) = loaded {
        crate::driving::settle(loaded).await;
    }

    // Started here, not found: magi convenes its siblings. Whichever program fills the `memory`
    // role, named after the key rather than the run — a run is shared by every agent in it, so a
    // socket named after one would refuse the second agent.
    let memory = loaded.map_or_else(
        || crate::config::roles::BALTHASAR.to_owned(),
        crate::config::memory,
    );
    let ours = crate::balthasar::start(&memory, key, Path::new(&cwd), agent).await;

    // The memory layer is the store, and there is no other. A JSONL fallback made two stores, one
    // of them going stale and silently — a session resumed from it resumes into something that half
    // happened. A session that cannot record is refused instead.
    let ours = match ours {
        crate::balthasar::Started::Ours(socket) => Some(socket),
        crate::balthasar::Started::Theirs => None,
        // Said in the words the attempt produced: refusing a session means naming what went wrong.
        crate::balthasar::Started::Refused(why) => {
            anyhow::bail!("{}", unreachable(&memory, "convene", &why))
        }
    };
    let dialled = match &ours {
        Some(socket) => magi_ipc::family::Family::dial(socket).await,
        None => magi_ipc::family::Family::find(None).await,
    };
    let family = dialled
        .map_err(|why| anyhow::anyhow!("{}", unreachable(&memory, "reach", &why.to_string())))?;
    let mut scribe = magi_host::scribe::Scribe::over(family, ours.clone(), &id);
    let carried = match resume.then(|| resumable(&mut scribe)) {
        Some(fut) => fut.await,
        None => Vec::new(),
    };
    let session = magi_host::session::Session::recorded(id, carried);
    // Nothing outlives its process, so a stale socket here was left by a crash and is cleared.
    if let Some(parent) = socket.parent() {
        tokio::fs::create_dir_all(parent).await?;
        sweep(parent);
    }
    let listener = magi_ipc::bind(socket)
        .await
        .with_context(|| format!("binding {}", socket.display()))?;

    // Asked once and handed to the session; melchior owns the catalog.
    let mut catalog = match loaded {
        Some(loaded) => crate::config::catalog(
            loaded,
            magi_host::broker::cards(&crate::config::mind(loaded)).await,
        ),
        None => magi_host::catalog::Catalog::empty(),
    };
    let mut backend = crate::config::backend(&catalog);
    stamp(&mut backend, &mut catalog, environ);
    tokio::spawn(async move {
        let _ = magi_host::serve_on(listener, session, backend, catalog, ours).await;
    });
    Ok(())
}

/// Take the socket back down: a path nothing answers would meet the next `magi` as a name taken.
pub fn done(socket: &Path) {
    let _ = std::fs::remove_file(socket);
    // And the directory, if this was the last session: `remove_dir` refuses a non-empty one.
    if let Some(parent) = socket.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

/// Clear out sockets in `dir` that nothing is serving, at startup rather than only at exit, because
/// what needs clearing is the sessions that never reached their exit path. Dialled, never guessed:
/// unlinking a path because it looks stale would take a live session's socket out from under it.
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

/// What balthasar files this session's history and its scratch under: melchior's run when there is
/// one, since that is the segment balthasar opens a scratch directory for and magi's key is a pid
/// and a clock. The key stays as the fallback for a session with no melchior. Nothing is moved.
fn recorded_as(run: Option<&str>, key: &str) -> String {
    match run.map(str::trim).filter(|run| !run.is_empty()) {
        Some(run) => run.to_owned(),
        None => magi_host::paths::session_id(unix_seconds(), key),
    }
}

/// Seconds since the epoch: a session id is a sortable timestamp, so "the most recent session" is
/// a directory listing rather than an index to maintain.
fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Put this session's environment where every process it starts will pick it up. Both, because
/// tools are built from the *backend* and a `/model` switch rebuilds a backend from the catalog.
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

/// Which name a session's history is kept under.
#[cfg(test)]
mod naming {
    use super::*;

    #[test]
    fn a_session_in_a_run_is_recorded_as_the_run() {
        // `<session>` in balthasar's scratch path is melchior's run, so every agent of one run
        // opens a directory beside its siblings'.
        assert_eq!(recorded_as(Some("alpha-rho"), "beef00042"), "alpha-rho");
    }

    #[test]
    fn a_session_with_no_melchior_falls_back_to_its_own_key() {
        // pid and clock, exactly as before: there is no run to belong to.
        for absent in [None, Some(""), Some("   ")] {
            let fallback = recorded_as(absent, "beef00042");
            assert!(
                fallback.ends_with("-beef00042"),
                "the key is what tells apart two sessions started in one second: {fallback}"
            );
        }
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
        catalog.cards = vec![magi_proto::ask::Card {
            id: "fake/m".into(),
            provider: "fake".into(),
            name: "m".into(),
            api: "openai-completions".into(),
            context_window: Some(1000),
            max_output: Some(100),
            reasons: false,
            ready: true,
            needs: None,
        }];
        catalog
    }

    #[test]
    fn the_session_s_name_reaches_the_tools_it_starts() {
        // A tool peer is spawned from the *backend*'s environment, so a name put only on the
        // catalog never reached it.
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
        // `/model` builds a fresh backend from the catalog, so a name only on the backend is lost.
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

    #[test]
    fn a_session_with_no_model_still_names_itself() {
        // Every `agent` verb works without one.
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
    use magi_model::scratch::Scratch;

    /// A socket file with nothing behind it, the way a session's corpse is left.
    ///
    /// `mknod` rather than a bind that is dropped: a `fork` on any other thread between the two
    /// copies the listening descriptor into the child, and until it `exec`s the kernel still
    /// accepts on the path. `mknod` opens no descriptor, so there is nothing to inherit.
    fn corpse(path: &Path) {
        rustix::fs::mknodat(
            rustix::fs::CWD,
            path,
            rustix::fs::FileType::Socket,
            rustix::fs::Mode::from_bits_truncate(0o600),
            0,
        )
        .expect("mknod a socket");
    }

    #[test]
    fn a_socket_nothing_answers_is_cleared_and_a_live_one_is_not() {
        // The directory is how a session is found, so litter in it is not cosmetic.
        let dir = Scratch::new("magi-sweep", "one");

        let live = std::os::unix::net::UnixListener::bind(dir.join("alive.host")).expect("bind");
        let dead = dir.join("dead.host");
        corpse(&dead);
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
    }

    #[test]
    fn the_last_session_out_takes_the_directory_with_it() {
        let dir = Scratch::new("magi-empty", "one");
        let socket = dir.join("one.host");
        std::fs::write(&socket, b"").expect("write");

        done(&socket);
        assert!(!dir.exists(), "an empty project directory was left behind");
    }

    #[test]
    fn a_directory_somebody_else_is_still_in_stays() {
        // The test is `remove_dir` refusing a directory that holds something.
        let dir = Scratch::new("magi-busy", "one");
        std::fs::write(dir.join("mine.host"), b"").expect("write");
        std::fs::write(dir.join("theirs.host"), b"").expect("write");

        done(&dir.join("mine.host"));
        assert!(
            dir.exists(),
            "a directory with a session still in it was removed"
        );
        assert!(dir.join("theirs.host").exists());
    }
}

/// Why a session is refused when the `memory` role's program will not answer. The program is named
/// rather than balthasar: a person who pointed `magi.memory` elsewhere is owed the name they chose.
fn unreachable(memory: &str, what: &str, why: &str) -> String {
    format!(
        "magi could not {what} {memory}, which holds this session's history: {why}\n\
         {memory} fills the `memory` role and is the store — there is no local journal to fall \
         back to. Install it and put it on PATH, or check `{memory} status`."
    )
}

/// The newest run balthasar holds for this project, for `--resume`. Empty is a fresh session.
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
