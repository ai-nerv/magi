//! The session: the transcript, the socket, and the turns. One session per process, running as a
//! task inside the `magi` that shows it — there is no daemon, so a session ends when its window
//! does. `UiCommand::Attach` names one, so a registry would be a lookup, not a protocol change.

pub mod asking;
pub mod broker;
pub mod cancel;
pub mod catalog;
pub mod compact;
pub mod context;
pub mod declaring;
pub mod driving;
pub mod holder;
pub mod injecting;
pub mod knowing;
pub mod paths;
pub mod remember;
pub mod scribe;
pub mod session;
pub mod supplying;
pub mod system;
pub mod turn;
pub mod worker;

use magi_ipc::{FrameReader, FrameWriter, IpcError, PeerCred};
use magi_journal::JournalError;
use magi_proto::{
    AgentStatus, Cursor, Entry, ErrorClass, HarnessEvent, MessageId, SessionId, StopReason,
    UiCommand,
};

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
    let Some((session, scribe)) = DRAINING.get() else {
        return;
    };
    if let Err(why) = crate::scribe::flush(session, &mut *scribe.lock().await).await {
        magi_model::noted!("drain: the last turn did not reach balthasar: {why}");
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

/// The same, able to change model without restarting. The catalog is everything this session
/// started with, held so a switch cannot silently pick up an edit made since.
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
    mut session: Session,
    backend: Option<turn::Backend>,
    catalog: crate::catalog::Catalog,
    balthasar: Option<std::path::PathBuf>,
) -> Result<(), HostError> {
    // Told once, here, because this is the only place that knows both.
    session.set_choices(catalog.choices());
    session.set_model(backend.as_ref().map(|backend| magi_proto::ModelInfo {
        name: backend.model.clone(),
        context_window: backend.context_window.unwrap_or(0),
    }));
    let session = Arc::new(Mutex::new(session));
    // Turns run on the worker's own thread because a protocol lives in a Lua VM; a session with no
    // backend has no worker. Behind a lock because `/model` replaces it — the worker owns a VM
    // built for one protocol. Shared with every connection, so any attached UI can answer.
    let pending = Arc::new(crate::asking::Pending::new());
    // Dialled once, and `None` when balthasar is not running, which is the ordinary case. Kept
    // before the block below takes it: the VM is told where to reach this session's balthasar.
    let told = balthasar.clone();
    let scribe = Arc::new(Mutex::new({
        let id = session.lock().await.id().clone();
        match balthasar {
            Some(path) => magi_ipc::family::Family::dial(&path)
                .await
                .ok()
                .map(|family| crate::scribe::Scribe::over(family, Some(path.clone()), &id)),
            None => crate::scribe::Scribe::find(&id).await.ok(),
        }
    }));
    let _ = DRAINING.set((Arc::clone(&session), Arc::clone(&scribe)));
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
        match installed(&mut catalog.clients, "balthasar", served) {
            Put::Kept => {}
            Put::Replaced => {
                magi_model::noted!("clients: balthasar's own library replaced this build's copy");
            }
            Put::Added => {
                magi_model::noted!("clients: balthasar's own library is this session's only copy");
            }
        }
    }
    // The asker publishes through the session's own broadcast handle rather than through the lock,
    // and "is anybody attached" is that channel's subscriber count. One asker, two traits: two
    // would be two ids counting from zero into one map. Built after the holding it draws on.
    let holding = Arc::new(crate::holder::Holding::new());
    // Questions a surface cannot answer itself go to a task of the session's own, not a connection's.
    let knows = {
        let (asking, asked) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(crate::knowing::serve(
            asked,
            Arc::clone(&scribe),
            Arc::clone(&session),
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
                magi_tools::casper::CASPER,
            )
            .knowing(Arc::clone(&knows) as Arc<dyn magi_tools::holding::Answers>),
        )
    };
    let asker = {
        let events = session.lock().await.publisher();
        let watched = events.clone();
        Arc::new(
            crate::asking::Asker::new(
                Arc::clone(&pending),
                Box::new(move |event| {
                    let _ = events.send(event);
                }),
                // Transient rather than journalled, and the UI tracks the highest cursor with a max.
                Box::new(|| Cursor::ZERO),
                Box::new(move || watched.receiver_count() > 0),
            )
            .drawn_by(Arc::clone(&holds)),
        )
    };
    let person = crate::asking::Person::of(asker, holds, Arc::clone(&holding));
    let worker = Arc::new(tokio::sync::RwLock::new(
        backend
            .map(|backend| {
                worker::Worker::gated(
                    backend,
                    Some(Arc::clone(&person.approver)),
                    Arc::clone(&person.asks),
                    Arc::clone(&person.holds),
                    Arc::clone(&scribe),
                )
            })
            .map(Arc::new),
    ));
    let catalog = Arc::new(catalog);
    // No idle timer. Nothing outlives its UI now, so there is nothing to sweep, and a UI whose
    // connection hiccuped would have had its own session close the socket underneath it.
    loop {
        let stream = listener.accept().await?.0;
        // A session serves one user; any other uid is refused rather than authenticated.
        match PeerCred::of(&stream) {
            Ok(cred) if cred.is_same_user() => {}
            _ => continue,
        }
        let session = Arc::clone(&session);
        let worker = Arc::clone(&worker);
        let catalog = Arc::clone(&catalog);
        let pending = Arc::clone(&pending);
        let person = person.clone();
        let scribe = Arc::clone(&scribe);
        tokio::spawn(async move {
            let _ = connection(
                stream, session, &worker, &catalog, &pending, &person, &scribe,
            )
            .await;
        });
    }
}

/// One attached UI.
async fn connection(
    stream: UnixStream,
    session: Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    pending: &crate::asking::Pending,
    person: &crate::asking::Person,
    scribe: &Arc<Mutex<Option<crate::scribe::Scribe>>>,
) -> Result<(), HostError> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    let (from, draws) = match reader.read::<UiCommand>().await? {
        UiCommand::Attach {
            from_cursor, draws, ..
        } => (from_cursor, draws),
        // Anything before an attach is a peer that does not speak the protocol.
        _ => return Ok(()),
    };
    // Whether anybody here can draw rows a tool asks for. Held for the life of the connection, so
    // a UI that goes away takes its screen with it.
    let _drawing = crate::holder::Drawing::attach(&person.surfaces, draws);

    // Subscribe before reading state, so an entry committed between the two is not lost in the gap.
    let (snapshot, backlog, mut live) = {
        let session = session.lock().await;
        (
            session.snapshot(from),
            session.replay(from),
            session.subscribe(),
        )
    };

    writer.write(&snapshot).await?;
    for event in backlog {
        writer.write(&event).await?;
    }

    // Commands are read in their own task because `FrameReader::read` is not cancel-safe: it takes
    // a length then a body, and a `select!` dropping it between the two parses body as a length.
    // The channel is what says the client has gone: the sender is dropped when this task returns,
    // so a closed connection reaches the loop as the queue draining and then `None`.
    let (commands, mut incoming) = tokio::sync::mpsc::channel::<UiCommand>(32);
    let reading = tokio::spawn(async move {
        while let Ok(command) = reader.read::<UiCommand>().await {
            if commands.send(command).await.is_err() {
                return;
            }
        }
    });

    loop {
        tokio::select! {
            command = incoming.recv() => {
                match command {
                    Some(UiCommand::SubmitPrompt { text, aside }) => {
                        let held = worker.read().await.clone();
                        submit(&session, Entry::User {
                            id: MessageId::new(format!("u{}", session.lock().await.cursor().next().0)),
                            text,
                            aside,
                        }, held, catalog, scribe).await?;
                    }
                    Some(UiCommand::Arrived { who, kin, sort, text }) => {
                        let arrived = Entry::From { who, kin, sort, text };
                        let wake = wants_answering(&arrived);
                        // Nothing another instance says interrupts a turn. What arrives now is
                        // dealt with when the turn it arrived during is over — see `after`.
                        if !session.lock().await.idle() {
                            session.lock().await.hold(arrived);
                            continue;
                        }
                        let held = worker.read().await.clone();
                        if wake {
                            // A turn, the same way a prompt starts one; without it the entry lands
                            // in the transcript and nothing reads it.
                            submit(&session, arrived, held, catalog, scribe).await?;
                        } else {
                            // Committed and no more: a note is something to have seen, not a
                            // reason to start answering.
                            session.lock().await.commit(arrived)?;
                        }
                    }
                    Some(UiCommand::TakeGrants { grants }) => {
                        let held = worker.read().await.clone();
                        if let Some(worker) = held {
                            // Queued like a turn, so one already running finishes under the
                            // permissions it started with.
                            worker.take_on(Arc::clone(&session), grants).await;
                        }
                    }
                    Some(UiCommand::DeclareNeeds) => {
                        let held = worker.read().await.clone();
                        if let Some(worker) = held {
                            // Spawned, because the declaration blocks on prompts answered by
                            // commands read on this very loop.
                            let session = Arc::clone(&session);
                            tokio::spawn(async move { worker.declare(session).await });
                        }
                    }
                    Some(UiCommand::SetModel { name }) => {
                        if let Some(refusal) =
                            switch_model(&session, worker, catalog, person, scribe, &name).await
                        {
                            // On the stream rather than in the transcript: a fact about the UI's
                            // ask, not about the conversation.
                            writer
                                .write(&HarnessEvent::Refused {
                                    cursor: session.lock().await.cursor(),
                                    message: refusal,
                                })
                                .await?;
                        }
                    }
                    Some(UiCommand::SetThinking { level }) => {
                        if let Some(refusal) =
                            switch_thinking(&session, worker, catalog, person, scribe, &level).await
                        {
                            writer
                                .write(&HarnessEvent::Refused {
                                    cursor: session.lock().await.cursor(),
                                    message: refusal,
                                })
                                .await?;
                        }
                    }
                    Some(UiCommand::Resume { id }) => {
                        // One place to ask: balthasar is the store, and a session it does not know
                        // does not exist.
                        let replayed = match scribe.lock().await.as_mut() {
                            Some(scribe) => scribe.replay_of(&id).await.ok(),
                            None => None,
                        };
                        let refusal = match replayed {
                            Some(entries) if !entries.is_empty() => {
                                session
                                    .lock()
                                    .await
                                    .resume_recorded(SessionId::new(id.clone()), entries);
                                None
                            }
                            _ => Some(format!("there is no session called {id:?}")),
                        };
                        if let Some(message) = refusal {
                            writer
                                .write(&HarnessEvent::Refused {
                                    cursor: session.lock().await.cursor(),
                                    message,
                                })
                                .await?;
                        }
                    }
                    Some(UiCommand::Branch { keeps }) => {
                        let mut held = session.lock().await;
                        if let Some(keeps) =
                            keeps.or_else(|| context::rewind_point(held.entries()))
                        {
                            // Journalled, not applied: the entries it skips are still there.
                            let id = MessageId::new(format!("b{}", held.cursor().next().0));
                            held.commit(Entry::Branch { id, keeps })?;
                        }
                    }
                    // Handed straight to whoever is blocked on it; an id nobody waits on is dropped.
                    Some(UiCommand::Permit { id, decision }) => {
                        pending.answer(&id, decision);
                    }
                    // The same, for a question a tool asked in its own words.
                    Some(UiCommand::Answered { id, choice }) => {
                        pending.chose(&id, choice);
                    }
                    // How much room the screen has. The session has no terminal of its own.
                    Some(UiCommand::Sized { rows, cols, holds }) => {
                        person.surfaces.sized(rows, cols, holds);
                    }
                    // A key aimed at rows a tool is holding, not interpreted on the way through.
                    Some(UiCommand::Keyed { id, key, state }) => {
                        person.surfaces.keyed(&id, key, state);
                    }
                    // The pointer, already in the surface's own coordinates: the UI translated it.
                    Some(UiCommand::Moused { id, kind, button, row, col }) => {
                        person.surfaces.moused(&id, kind, button, row, col);
                    }
                    Some(UiCommand::Interrupt) => {
                        // Set here as well as by the turn, so a stop shows at once rather than
                        // once the provider notices.
                        let held = session.lock().await;
                        held.cancel().request();
                        drop(held);
                        session.lock().await.set_status(AgentStatus::Idle);
                    }
                    Some(UiCommand::Attach { .. }) => {}
                    Some(UiCommand::Detach) | None => break,
                }
            }
            event = live.recv() => {
                match event {
                    // Awaited in the branch body, not as a select arm: a cancelled write desyncs.
                    Ok(event) => writer.write(&event).await?,
                    // A UI that fell behind reattaches with its cursor; the journal holds it all.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        }
    }

    reading.abort();
    Ok(())
}
#[path = "switching.rs"]
mod switching;
use switching::{switch_model, switch_thinking};
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
        "question" | "answer" | "attention" | "trouble" | "handoff"
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
