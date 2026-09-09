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

/// The three names a session opens under.
///
/// One parameter rather than three because they are one fact between them, and because two of
/// the three are `Option<&str>`: a call site that swapped those would compile, and would file
/// every agent's scratch under the run.
pub struct Named<'a> {
    /// This process's own, for the two files nothing else may share: its host socket, and the
    /// balthasar it convenes.
    pub key: &'a str,
    /// melchior's run, which balthasar files a history under — see [`recorded_as`].
    pub run: Option<&'a str>,
    /// Which agent of that run this is, which balthasar files scratch under.
    pub agent: Option<&'a str>,
}

/// Open this session and start serving it, without waiting for it to finish.
///
/// Bound before returning, so the UI's first dial cannot race the bind. Everything after that
/// is a task: the caller goes on to draw.
///
/// `resume` continues this directory's most recent session instead of starting one. What that
/// means is balthasar's to answer — see [`resumable`].
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
    // Told before started. A sibling reads what a coordinator said as it comes up, so saying it
    // afterwards would configure the turn after this one.
    if let Some(loaded) = loaded {
        crate::driving::settle(loaded).await;
    }

    // Started here, not found. magi convenes its siblings: a session whose transcript depended
    // on somebody else having launched a memory layer would record sometimes and not others.
    //
    // **Named after the key, not after the run.** These two were one value, and separating them
    // is what lets several agents of one run each convene their own layer: the run is shared by
    // every agent in it by design, so a socket named after it would have the second agent's
    // `balthasar serve` refused the address the first is holding — a subagent that will not
    // start, reported as "balthasar could not be convened". The key is one process's own.
    let ours = crate::balthasar::start(key, Path::new(&cwd), agent).await;

    // **balthasar is the store, and there is no other.** This used to fall back to a JSONL file
    // per session when it could not be reached, and that fallback was the bug: two stores is one
    // store and a copy that goes stale, a session resumed from the stale one resumes into
    // something that half-happened, and — because the fallback was silent — nobody could tell
    // which of the two they had been using. A session that cannot record is refused instead.
    //
    // Refusing is affordable precisely because magi *convenes* balthasar rather than finding it:
    // reaching here with no balthasar means the binary is missing or would not start, which is a
    // thing to say out loud rather than to work around.
    let ours = match ours {
        crate::balthasar::Started::Ours(socket) => Some(socket),
        crate::balthasar::Started::Theirs => None,
        // Said in the words the attempt produced, rather than in a guess made here. This was a
        // debug log and an `Option`, which was fine when the answer to every cause was "keep a
        // journal instead"; refusing a session means naming what actually went wrong.
        crate::balthasar::Started::Refused(why) => {
            anyhow::bail!(
                "magi could not convene balthasar, which holds this session's history: {why}\n\
                 balthasar is the store — there is no local journal to fall back to. \
                 Install it and put it on PATH, or check `balthasar status`."
            )
        }
    };
    let dialled = match &ours {
        Some(socket) => magi_ipc::family::Family::dial(socket).await,
        None => magi_ipc::family::Family::find(None).await,
    };
    let family = dialled.map_err(|why| {
        anyhow::anyhow!(
            "magi could not reach balthasar, which holds this session's history: {why}\n\
             balthasar is the store — there is no local journal to fall back to. \
             Install it and put it on PATH, or check `balthasar status`."
        )
    })?;
    let mut scribe = magi_host::scribe::Scribe::over(family, ours.clone(), &id);
    let carried = match resume.then(|| resumable(&mut scribe)) {
        Some(fut) => fut.await,
        None => Vec::new(),
    };
    let session = magi_host::session::Session::recorded(id, carried);
    // A stale socket cannot be a running session any more — nothing outlives its process — so
    // one found here was left by a crash and is cleared rather than treated as somebody's.
    if let Some(parent) = socket.parent() {
        tokio::fs::create_dir_all(parent).await?;
        sweep(parent);
    }
    let listener = magi_ipc::bind(socket)
        .await
        .with_context(|| format!("binding {}", socket.display()))?;

    // Asked once, here, and handed to the session. melchior owns the catalog; a session that
    // re-read it per switch would answer with a model the person did not choose.
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

/// What balthasar files this session's history and its scratch under.
///
/// **melchior's run, when there is one.** A run is the root main and everything under it, frozen
/// at birth, and it is the segment balthasar opens a scratch directory for —
/// `<project>/balthasar/<tool>/<run>/<agent>/memory.db`. magi's key cannot be that name: it is
/// the pid and the clock, so it is a different value in every process, and a coordinator and its
/// subagents would each file their memory in a directory none of the others could name.
///
/// The key stays as the fallback, for a session with no melchior. There is no run to belong to
/// then, and a name unique to this process is what "one directory per run" already means.
///
/// Existing histories keep the names they were written under. Nothing is moved.
fn recorded_as(run: Option<&str>, key: &str) -> String {
    match run.map(str::trim).filter(|run| !run.is_empty()) {
        Some(run) => run.to_owned(),
        None => magi_host::paths::session_id(unix_seconds(), key),
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

/// Which name a session's history is kept under.
#[cfg(test)]
mod naming {
    use super::*;

    #[test]
    fn a_session_in_a_run_is_recorded_as_the_run() {
        // The whole of stage 2's second half: `<session>` in balthasar's scratch path is
        // melchior's run, so every agent of one run opens a directory beside its siblings'
        // rather than one nothing else can name.
        assert_eq!(recorded_as(Some("alpha-rho"), "beef00042"), "alpha-rho");
    }

    #[test]
    fn a_session_with_no_melchior_falls_back_to_its_own_key() {
        // pid and clock, exactly as before. There is no run to belong to, and inventing one
        // would file this session's memory under a name that does not exist.
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
    use magi_model::scratch::Scratch;

    #[test]
    fn a_socket_nothing_answers_is_cleared_and_a_live_one_is_not() {
        // Ten of these had collected in one project, from crashes and from a build that named
        // its socket differently, and nothing would ever have removed them. The directory is how
        // a session is found, so litter in it is not cosmetic.
        let dir = Scratch::new("magi-sweep", "one");

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
        // The test is `remove_dir` refusing a directory that holds something, which is what
        // makes this safe without a listing and without racing a session that is binding.
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
