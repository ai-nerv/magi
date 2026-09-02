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
    me: &crate::instance::Identity,
) -> Result<()> {
    let dir = sessions.map_or_else(axon_host::paths::sessions_dir, Path::to_path_buf);
    let cwd = cwd.display().to_string();
    let session = match resume.then(|| free(&dir, &cwd, me)).flatten() {
        Some(path) => axon_host::session::Session::open(
            &path,
            axon_proto::SessionId::new(axon_host::paths::session_id(unix_seconds(), &me.id)),
            &cwd,
            unix_seconds(),
        )?,
        None => axon_host::open_session(&dir, &cwd, unix_seconds(), &me.id)?,
    };
    // A stale socket cannot be a running session any more — nothing outlives its process — so
    // one found here was left by a crash and is cleared rather than treated as somebody's.
    if let Some(parent) = socket.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = axon_ipc::bind(socket)
        .await
        .with_context(|| format!("binding {}", socket.display()))?;

    let mut backend = loaded.and_then(crate::config::backend);
    let mut catalog =
        loaded.map_or_else(axon_host::catalog::Catalog::empty, crate::config::catalog);
    stamp(&mut backend, &mut catalog, environ);
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

/// The newest journal for `cwd` that nothing is still writing to.
///
/// `--resume` used to mean "the newest one here", full stop, and that was fine while a directory
/// had one session. It does not any more: two `axon -r` in one project both took the newest,
/// both opened it, and appended into one file in whatever order they happened to write — a
/// transcript neither of them said.
///
/// A journal is named after the session that made it and a session id ends in the instance's
/// name, so "is anybody still writing this" is a question the runtime directory answers: if that
/// instance's socket is still up, the journal is taken. `None` means every one of them is, which
/// is a fresh session rather than a refusal — somebody asking to resume wants to start working.
fn free(dir: &Path, cwd: &str, me: &crate::instance::Identity) -> Option<std::path::PathBuf> {
    axon_host::paths::summaries(dir, cwd)
        .into_iter()
        .find(|session| {
            let Some(whose) = session.id.split_once('-').map(|(_, name)| name) else {
                // A journal from before session ids carried a name. Nothing can be checked, and
                // the old behaviour is the right one for it.
                return true;
            };
            !crate::instance::asking::answers(&crate::instance::socket(&me.project, whose), me)
        })
        .map(|session| session.path)
}

/// Put this session's environment where every process it starts will pick it up.
///
/// **Both, and the reason is not symmetry.** Tools are built from the *backend*, so stamping the
/// catalog alone left the `agent` peer with no name: `mine()` answered `None` and every verb
/// refused with "this process was not started by an axon session" — a session reachable by name
/// that could reach nobody. And the catalog is what a `/model` switch rebuilds a backend from,
/// so stamping the backend alone would have worked right up until somebody changed model.
fn stamp(
    backend: &mut Option<axon_host::turn::Backend>,
    catalog: &mut axon_host::catalog::Catalog,
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
    use crate::instance;

    fn environ() -> std::collections::BTreeMap<String, String> {
        [
            (instance::PROJECT.to_owned(), "axon".to_owned()),
            (instance::ROLE.to_owned(), "main".to_owned()),
            (instance::ID.to_owned(), "delta-rho".to_owned()),
        ]
        .into_iter()
        .collect()
    }

    /// A catalog holding one model that needs no credential, so it yields a real backend.
    fn catalog() -> axon_host::catalog::Catalog {
        let mut catalog = axon_host::catalog::Catalog::empty();
        catalog.providers = vec![axon_provider::provider::Provider {
            id: "fake".into(),
            name: "Fake".into(),
            base_url: Some("http://127.0.0.1:1/v1".into()),
            api: axon_provider::model::Api::OpenAiCompletions,
            auth: axon_provider::provider::Auth::None,
            compat: None,
            models: vec![axon_provider::model::Model {
                id: "m".into(),
                provider: "fake".into(),
                name: "M".into(),
                api: axon_provider::model::Api::OpenAiCompletions,
                reasoning: false,
                input: axon_provider::model::default_input(),
                context_window: 1000,
                max_tokens: 100,
                cost: axon_model::Cost::default(),
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
        // verb with "this process was not started by an axon session" — a session reachable by
        // name that could reach nobody.
        let mut catalog = catalog();
        let mut backend = catalog.backend("fake/m");
        assert!(backend.is_some(), "the fixture yields a backend");
        stamp(&mut backend, &mut catalog, &environ());

        let started = backend.expect("a backend").environ;
        assert_eq!(
            started.get(instance::ID).map(String::as_str),
            Some("delta-rho")
        );
        assert_eq!(
            started.get(instance::PROJECT).map(String::as_str),
            Some("axon")
        );
        assert_eq!(
            started.get(instance::ROLE).map(String::as_str),
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
            after.get(instance::ID).map(String::as_str),
            Some("delta-rho"),
            "a switch lost the session's name"
        );
    }

    /// A session in a project nothing is running in, so no journal reads as taken.
    fn nobody() -> crate::instance::Identity {
        crate::instance::Identity {
            project: "no-such-project-here".to_owned(),
            role: "main".to_owned(),
            id: "alpha-rho".to_owned(),
        }
    }

    /// A journal in `dir` for `cwd`, named after the session that made it.
    fn journal(dir: &Path, id: &str, cwd: &str) {
        let path = dir.join(format!("{id}.jsonl"));
        axon_journal::Journal::open(&path, axon_proto::SessionId::new(id.to_owned()), cwd, 1)
            .expect("journal");
    }

    #[test]
    fn resuming_takes_the_newest_journal_nobody_is_writing_to() {
        // Two `axon -r` in one project both used to take the newest, both open it, and append
        // into one file in whatever order they happened to write.
        let dir = std::env::temp_dir().join(format!("axon-free-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        journal(&dir, "00000000000000000001-alpha-rho", "/work");
        journal(&dir, "00000000000000000002-beta-nu", "/work");

        // Nothing is listening in either name, so the newest wins as it always did.
        let found = free(&dir, "/work", &nobody()).expect("a journal");
        assert!(found.to_string_lossy().contains("beta-nu"), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_journal_from_before_names_is_still_resumable() {
        // Written when a session id was a bare timestamp. Nothing can be checked about it, and
        // refusing to resume it would lose somebody their history over a naming change.
        let dir = std::env::temp_dir().join(format!("axon-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        journal(&dir, "00000000000000000007", "/work");
        assert!(free(&dir, "/work", &nobody()).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_to_resume_is_a_fresh_session_rather_than_a_refusal() {
        // Somebody asking to resume wants to start working.
        let dir = std::env::temp_dir().join(format!("axon-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(free(&dir, "/work", &nobody()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_with_no_model_still_names_itself() {
        // Every `agent` verb works without one, and a session that cannot answer a prompt can
        // still be asked what it is doing.
        let mut catalog = axon_host::catalog::Catalog::empty();
        let mut nothing = None;
        stamp(&mut nothing, &mut catalog, &environ());
        assert!(nothing.is_none());
        assert_eq!(
            catalog.environ.get(instance::ID).map(String::as_str),
            Some("delta-rho")
        );
    }
}
