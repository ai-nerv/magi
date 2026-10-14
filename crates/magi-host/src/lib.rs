//! The session: the transcript, the socket, and the turns. One session per process, running as a
//! task inside the `magi` that shows it — there is no daemon, so a session ends when its window
//! does. `UiCommand::Attach` names one, so a registry would be a lookup, not a protocol change.

pub mod admitting;
pub mod asking;
pub mod broker;
pub mod cancel;
pub mod catalog;
mod catching;
pub mod context;
pub mod declaring;
pub mod driving;
pub mod helping;
pub mod holder;
pub mod judging;
pub mod knowing;
pub mod laying;
pub mod paths;
pub mod remember;
mod reporting;
mod resuming;
pub mod scribe;
mod sending;
pub mod session;
mod settling;
pub mod system;
pub mod turn;
pub mod worker;

use magi_ipc::{FrameReader, FrameWriter, IpcError, PeerCred};
use magi_journal::JournalError;
use magi_proto::{Cursor, Entry, ErrorClass, HarnessEvent, MessageId, StopReason, UiCommand};

use session::Session;

use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

/// How long balthasar has to answer a question asked while the session is starting. Nothing asked
/// here is needed for the session to run, and a slow one must not hold up serving its own socket.
const GREETING: std::time::Duration = std::time::Duration::from_millis(500);

/// What [`drain`] needs to reach, set once the session is serving. A process-global because the
/// process is one session; a turn's flush runs on a spawned task the process can exit before.
type Draining = (
    Arc<Mutex<Session>>,
    Arc<Mutex<Option<crate::scribe::Scribe>>>,
);
static DRAINING: std::sync::OnceLock<Draining> = std::sync::OnceLock::new();

/// Hand over anything a turn settled that has not reached balthasar yet. Called on the way out,
/// after the last turn and before the socket goes. A failure is logged rather than returned: the
/// last exchange is then missing from the store, and the next run's `--resume` comes back short.
pub async fn drain() {
    if let Some((session, scribe)) = DRAINING.get() {
        let helpers = session.lock().await.helpers();
        if let Ok(paused) = helpers.pause()
            && let Err(why) = paused.drain(std::time::Duration::from_secs(30)).await
        {
            magi_model::noted!("drain: {why}");
        }
        if let Err(why) = crate::scribe::flush(session, &mut *scribe.lock().await).await {
            magi_model::noted!("drain: the last turn did not reach balthasar: {why}");
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error(transparent)]
    Ipc(#[from] IpcError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Journal(#[from] JournalError),

    /// A request was measured at the boundary and does not fit what the model leaves. Nothing was
    /// sent, and nothing was cut down to make it fit: what to leave out is the memory layer's.
    #[error("{0}")]
    Refused(String),
}

/// Serve one session until cancelled. Every connection gets its own task; the session is shared
/// behind a mutex held for a journal append and released, never across a provider call.
pub async fn serve(
    listener: UnixListener,
    session: Session,
    backend: Option<turn::Backend>,
) -> Result<(), HostError> {
    serve_catalog(listener, session, backend, crate::catalog::Catalog::empty()).await
}

/// What became of a served client library.
#[derive(Debug, PartialEq, Eq)]
enum Put {
    /// The catalog already had exactly this.
    Kept,
    /// The catalog had an older copy under that name.
    Replaced,
    /// The catalog had no copy at all.
    Added,
}

/// Put the library a sibling serves into the catalog under `name`.
///
/// **Added when absent, not only replaced when present.** `clients` holds what `clients/*.lua` put
/// on disk, and no build has ever shipped a `clients/balthasar.lua` — so replace-only found nothing
/// to replace and dropped the served library. `config/tools.lua` reads `magi.clients.balthasar` to
/// declare `remember`, `recall` and `forget`, so with it nil that block registered nothing and the
/// model had no memory verbs, in sessions that had convened a balthasar and were recording to it.
fn installed(clients: &mut Vec<(String, String)>, name: &str, served: String) -> Put {
    match clients.iter_mut().find(|(held, _)| held == name) {
        Some((_, source)) if *source == served => Put::Kept,
        Some((_, source)) => {
            *source = served;
            Put::Replaced
        }
        None => {
            clients.push((name.to_owned(), served));
            Put::Added
        }
    }
}

/// The same, able to switch models and refresh cards without changing session configuration.
pub async fn serve_catalog(
    listener: UnixListener,
    session: Session,
    backend: Option<turn::Backend>,
    catalog: crate::catalog::Catalog,
) -> Result<(), HostError> {
    serve_on(listener, session, backend, catalog, None).await
}

/// The same, told which balthasar to record into. A path rather than a search: the newest socket
/// in the directory is a neighbour's as often as not.
pub async fn serve_on(
    listener: UnixListener,
    session: Session,
    backend: Option<turn::Backend>,
    catalog: crate::catalog::Catalog,
    balthasar: Option<std::path::PathBuf>,
) -> Result<(), HostError> {
    let writing = serving::WRITING;
    serve_writing(listener, session, backend, catalog, balthasar, writing).await
}

/// The same, letting a screen go when it has not taken an event within `writing`.
async fn serve_writing(
    listener: UnixListener,
    mut session: Session,
    backend: Option<turn::Backend>,
    catalog: crate::catalog::Catalog,
    balthasar: Option<std::path::PathBuf>,
    writing: std::time::Duration,
) -> Result<(), HostError> {
    // Told once, here, because this is the only place that knows both.
    session.set_choices(catalog.choices());
    session.run_helpers_on(tokio::runtime::Handle::current());
    session.set_model(backend.as_ref().map(|backend| magi_proto::ModelInfo {
        name: backend.model.clone(),
        context_window: backend.context_window.unwrap_or(0),
    }));
    let session = Arc::new(Mutex::new(session));
    // Turns run on the worker's own thread because a protocol lives in a Lua VM; a session with no
    // backend has no worker. Behind a lock because `/model` replaces it — the worker owns a VM
    // built for one protocol. Shared with every connection, so any attached UI can answer.
    let pending = Arc::new(crate::asking::Pending::new());
    // `None` only when no memory layer was named at all. One that was named and does not answer
    // yet is dialled again, backing off, by every call that needs it.
    let told = balthasar.clone();
    if let Some(path) = &balthasar {
        magi_ipc::family::own(path);
    }
    let scribe = Arc::new(Mutex::new({
        let id = session.lock().await.id().clone();
        let found = match balthasar.or_else(magi_ipc::family::named) {
            Some(path) => Some(match magi_ipc::family::Family::dial(&path).await {
                Ok(family) => crate::scribe::Scribe::over(family, Some(path), &id),
                Err(why) => {
                    magi_model::noted!("scribe: {why}; dialling again as it is needed");
                    crate::scribe::Scribe::unconnected(path, &id)
                }
            }),
            None => None,
        };
        found.map(|scribe| match &catalog.transcript {
            Some(key) => scribe.recording_as(key.clone()),
            None => scribe,
        })
    }));
    let _ = DRAINING.set((Arc::clone(&session), Arc::clone(&scribe)));
    magi_model::noted!(
        "scribe: recording as {}",
        catalog
            .transcript
            .as_deref()
            .unwrap_or("the session itself")
    );
    magi_model::noted!(
        "session: {} as agent {} serving in {}, model {}, memory {}",
        session.lock().await.id(),
        backend
            .as_ref()
            .and_then(|b| b.environ.get("MAGI_MELCHIOR_ID"))
            .map_or("-", String::as_str),
        catalog.cwd.display(),
        backend.as_ref().map_or("none", |b| b.model.as_str()),
        if scribe.lock().await.is_some() {
            "reached"
        } else {
            "absent"
        }
    );
    // Named for the VM, so a tool can say which session it is asking about.
    magi_lua::name_session(session.lock().await.id().as_str(), told.as_deref());

    // The library balthasar ships, in place of the copy this build carries: a consumer keeping its
    // own copy is one whose copy goes stale. Beside it, a cross-check — a balthasar holding fewer
    // turns than magi has entries has an incomplete scrollback. Both are on a clock, because this
    // runs before the socket is served and neither is needed for the session to start.
    {
        let held = session.lock().await.entries().len();
        let theirs = tokio::time::timeout(GREETING, async {
            let mut open = scribe.lock().await;
            match open.as_mut() {
                Some(open) => open.resumes().await.ok(),
                None => None,
            }
        })
        .await
        .ok()
        .flatten();
        if let Some(theirs) = theirs
            && held > 0
            && theirs == 0
        {
            magi_model::noted!(
                "scribe: this session has {held} entries and balthasar holds none of them; \
                 anything computed from its scrollback is about a different conversation"
            );
        }
    }
    // What balthasar is compacting for: the window size cannot be guessed from the turns, and
    // without this every plan fell back to its default of 200,000. On the same clock as the rest.
    if let Some(backend) = backend.as_ref()
        && let Some(window) = backend.context_window
    {
        let model = backend.model.clone();
        let _ = tokio::time::timeout(GREETING, async {
            let mut open = scribe.lock().await;
            match open.as_mut() {
                Some(open) => open.note_model(&model, window).await.ok(),
                None => None,
            }
        })
        .await;
    }

    let mut catalog = catalog;
    let served = tokio::time::timeout(GREETING, async {
        let mut open = scribe.lock().await;
        match open.as_mut() {
            Some(open) => open.library().await.ok(),
            None => None,
        }
    })
    .await
    .ok()
    .flatten();
    if let Some(served) = served {
        // Filed under the program's own name, because that is the key a tool description looks it
        // up by — see `magi.roles` and `config/tools.lua`.
        let memory = catalog.memory.clone();
        match installed(&mut catalog.clients, &memory, served) {
            Put::Kept => {}
            Put::Replaced => {
                magi_model::noted!("clients: {memory}'s own library replaced this build's copy");
            }
            Put::Added => {
                magi_model::noted!("clients: {memory}'s own library is this session's only copy");
            }
        }
    }
    // The asker publishes through the session's own broadcast handle rather than through the lock,
    // and "is anybody attached" is that channel's subscriber count. One asker, two traits: two
    // would be two ids counting from zero into one map. Built after the holding it draws on.
    let holding = Arc::new(crate::holder::Holding::new());
    // Questions a surface cannot answer itself go to a task of the session's own, not a connection's.
    // The same channel a surface asks down: the second model is one more thing to ask it for.
    let (asking, asked) = tokio::sync::mpsc::unbounded_channel();
    let judge_asks = asking.clone();
    let knows = {
        tokio::spawn(crate::knowing::serve(
            asked,
            Arc::clone(&scribe),
            Arc::clone(&session),
            backend.clone(),
        ));
        let held = session.lock().await;
        let cwd = catalog.cwd.display().to_string();
        Arc::new(crate::knowing::Knows::of(held.id(), &cwd).asking(asking))
    };
    let holds: Arc<dyn magi_tools::holding::Holds> = {
        let events = session.lock().await.publisher();
        let watched = events.clone();
        Arc::new(
            crate::holder::Holder::new(
                Arc::clone(&holding),
                Box::new(move |event| {
                    let _ = events.send(event);
                }),
                Box::new(move || watched.receiver_count() > 0),
                // The program the `tools` role named, because a surface is held by whatever
                // supplied the tool that asked for one.
                &catalog.tooling.program,
            )
            .knowing(Arc::clone(&knows) as Arc<dyn magi_tools::holding::Answers>),
        )
    };
    let asker = {
        let events = session.lock().await.publisher();
        let watched = events.clone();
        Arc::new(crate::asking::Asker::new(
            Arc::clone(&pending),
            Box::new(move |event| {
                let _ = events.send(event);
            }),
            // Transient rather than journalled, and the UI tracks the highest cursor with a max.
            Box::new(|| Cursor::ZERO),
            Box::new(move || watched.receiver_count() > 0),
        ))
    };
    let person = crate::asking::Person::of(
        asker,
        holds,
        Arc::clone(&knows) as Arc<dyn magi_tools::holding::Answers>,
        Arc::clone(&holding),
    );
    let person = guard(person, judge_asks, &catalog, &session).await;
    let worker = Arc::new(tokio::sync::RwLock::new(
        backend
            .map(|backend| {
                worker::Worker::gated(
                    backend,
                    Some(Arc::clone(&person.approver)),
                    Arc::clone(&person.asks),
                    Arc::clone(&person.holds),
                    Arc::clone(&person.knows),
                    Arc::clone(&scribe),
                )
            })
            .map(Arc::new),
    ));
    let served = Served {
        session,
        worker,
        models: Arc::new(refreshing::Models::new(catalog)),
        pending,
        person,
        scribe,
        key: controller_key(&listener)?,
        writing,
    };
    // No idle timer. Nothing outlives its UI now, so there is nothing to sweep, and a UI whose
    // connection hiccuped would have had its own session close the socket underneath it.
    let seats = Arc::new(tokio::sync::Semaphore::new(serving::SEATS));
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(why) if magi_ipc::transient(&why) => {
                magi_model::noted!("session: accept failed for now: {why}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
            Err(why) => {
                magi_model::noted!("session: the socket stopped accepting: {why}");
                return Err(why.into());
            }
        };
        // A session serves one user; any other uid is refused rather than authenticated.
        match PeerCred::of(&stream) {
            Ok(cred) if cred.is_same_user() => {}
            _ => continue,
        }
        let Ok(seat) = Arc::clone(&seats).try_acquire_owned() else {
            tokio::spawn(serving::busy(stream));
            continue;
        };
        let served = served.clone();
        tokio::spawn(async move {
            let _seat = seat;
            let _ = connection(stream, served).await;
        });
    }
}

mod refreshing;
mod serving;
use serving::{Served, connection, controller_key};
#[path = "switching.rs"]
mod switching;
use switching::{switch_model, switch_provider, switch_thinking};
// Re-exported: it answers "why is nothing configured", which the UI asks at attach.
pub use switching::no_model;
#[path = "turning.rs"]
mod turning;
use turning::submit;

/// Whether a message that arrived is one the session should answer rather than merely have read.
/// The sender chose, by which verb they used. Not the same question as "may this interrupt", which
/// the layer decides: this is an idle session, and answering it too narrowly is silent. `claim` and
/// `release` stay off it on purpose. The one place that decides — it was two, in two vocabularies.
#[must_use]
pub fn wants_answering(entry: &Entry) -> bool {
    matches!(entry, Entry::From { sort, .. } if matches!(
        sort.as_str(),
        "question" | "answer" | "attention" | "trouble" | "handoff" | "report"
    ))
}

/// Publish an error to whoever is attached.
#[must_use]
pub fn error_event(cursor: Cursor, class: ErrorClass, message: String) -> HarnessEvent {
    HarnessEvent::Error {
        cursor,
        class,
        message,
    }
}

/// A fresh session, named for the moment it started. Empty because it is new.
#[must_use]
pub fn open_session(now: u64, whose: &str) -> Session {
    Session::recorded(
        magi_proto::SessionId::new(paths::session_id(now, whose)),
        Vec::new(),
    )
}

/// Put the rules, the mode and the second model in front of whoever a session's tools ask. The
/// person is still there behind them, and is who every doubt comes back to.
async fn guard(
    mut person: crate::asking::Person,
    asking: tokio::sync::mpsc::UnboundedSender<crate::knowing::Wondering>,
    catalog: &crate::catalog::Catalog,
    session: &tokio::sync::Mutex<crate::session::Session>,
) -> crate::asking::Person {
    let standing = Arc::new(judging::Standing::starting(catalog.mode));
    let events = {
        let mut held = session.lock().await;
        held.judging = judging::described_by(catalog, &standing);
        held.publisher()
    };
    person.approver = Arc::new(judging::Judged::new(
        Arc::clone(&person.approver),
        Arc::new(judging::Helper::new(asking, &catalog.cwd)),
        Arc::clone(&standing),
        catalog.rules.clone(),
        &catalog.cwd,
        Box::new(move |text| {
            let _ = events.send(magi_proto::HarnessEvent::Noticed {
                cursor: magi_proto::Cursor::ZERO,
                text,
            });
        }),
    ));
    person.standing = standing;
    person
}

#[cfg(test)]
mod no_model_tests {
    use crate::catalog::Catalog;

    /// A catalog naming a model whose provider has no credential.
    fn wanting(name: &str) -> Catalog {
        Catalog {
            chosen: Some(name.to_owned()),
            cards: vec![magi_proto::ask::Card {
                id: "paid/x".to_owned(),
                provider: "Paid Co".to_owned(),
                name: "x".to_owned(),
                api: "openai-completions".to_owned(),
                context_window: Some(1000),
                max_output: Some(100),
                reasons: false,
                ready: false,
                needs: Some("MAGI_TEST_NOT_SET".to_owned()),
            }],
            ..Catalog::empty()
        }
    }

    #[test]
    fn a_configured_model_with_no_key_is_not_called_unconfigured() {
        // "No model is configured" on a machine whose `magi.model` was set and whose key was not.
        let said = super::no_model(&wanting("paid/x"));
        assert!(said.contains("MAGI_TEST_NOT_SET"), "{said}");
        assert!(!said.contains("No model is configured"), "{said}");
    }

    #[test]
    fn the_model_is_named_once() {
        let said = super::no_model(&wanting("paid/x"));
        assert_eq!(said.matches("paid/x").count(), 1, "{said}");
    }

    #[test]
    fn nothing_chosen_is_still_nothing_configured() {
        assert!(super::no_model(&Catalog::empty()).contains("No model is configured"));
    }
}

#[cfg(test)]
mod installing {
    use super::{Put, installed};

    #[test]
    fn a_library_no_client_file_declares_is_added_rather_than_dropped() {
        // The bug: `clients/*.lua` never declared a balthasar, so a replace-only pass found
        // nothing, discarded what balthasar served, and `config/tools.lua` registered no memory
        // verbs at all.
        let mut clients = vec![("oslo".to_owned(), "-- oslo".to_owned())];
        assert_eq!(
            installed(&mut clients, "balthasar", "-- served".to_owned()),
            Put::Added
        );
        assert_eq!(
            clients.iter().find(|(name, _)| name == "balthasar"),
            Some(&("balthasar".to_owned(), "-- served".to_owned()))
        );
    }

    #[test]
    fn a_stale_copy_on_disk_is_replaced_by_what_the_sibling_serves() {
        let mut clients = vec![("balthasar".to_owned(), "-- stale".to_owned())];
        assert_eq!(
            installed(&mut clients, "balthasar", "-- served".to_owned()),
            Put::Replaced
        );
        assert_eq!(clients[0].1, "-- served");
        assert_eq!(clients.len(), 1, "replacing does not also add");
    }

    #[test]
    fn a_copy_that_already_matches_is_left_alone() {
        // So the log line about replacing means a replacement happened.
        let mut clients = vec![("balthasar".to_owned(), "-- served".to_owned())];
        assert_eq!(
            installed(&mut clients, "balthasar", "-- served".to_owned()),
            Put::Kept
        );
        assert_eq!(clients.len(), 1);
    }

    #[test]
    fn nothing_else_in_the_catalog_moves() {
        let mut clients = vec![
            ("hexe".to_owned(), "-- hexe".to_owned()),
            ("oslo".to_owned(), "-- oslo".to_owned()),
        ];
        installed(&mut clients, "balthasar", "-- served".to_owned());
        assert_eq!(clients[0], ("hexe".to_owned(), "-- hexe".to_owned()));
        assert_eq!(clients[1], ("oslo".to_owned(), "-- oslo".to_owned()));
    }
}
