//! The axum daemon.
//!
//! Owns the session, the journal, and the socket. Answers the same protocol the replay host
//! answers, which is the whole test of M1: `axum host` stands in for `axum fake-host` without
//! a line of the UI moving.
//!
//! One session per daemon, for now. `UiCommand::Attach` already names one, so growing to a
//! registry is a lookup rather than a protocol change.

pub mod asking;
pub mod cancel;
pub mod catalog;
pub mod compact;
pub mod context;
pub mod declaring;
pub mod paths;
pub mod remember;
pub mod session;
pub mod system;
pub mod turn;
pub mod worker;

use axum_ipc::{FrameReader, FrameWriter, IpcError, PeerCred};
use axum_journal::JournalError;
use axum_proto::{
    AgentStatus, Cursor, Entry, ErrorClass, HarnessEvent, MessageId, StopReason, UiCommand,
};

use session::Session;
use std::path::Path;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

/// Anything that stops the daemon.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The transport failed.
    #[error(transparent)]
    Ipc(#[from] IpcError),

    /// Accepting a connection failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The journal could not be opened or written.
    #[error(transparent)]
    Journal(#[from] JournalError),
}

/// Serve one session until cancelled.
///
/// Every connection gets its own task; the session is shared behind a mutex because the log is
/// the one thing that must serialize. That is Tau's "commit chokepoint" without Tau's daemon:
/// the lock is held for a journal append and released, never across a provider call.
pub async fn serve(
    listener: UnixListener,
    session: Session,
    backend: Option<turn::Backend>,
) -> Result<(), HostError> {
    serve_catalog(listener, session, backend, crate::catalog::Catalog::empty()).await
}

/// The same, able to change model without restarting.
///
/// The catalog is what `/model` picks among: everything this session started with, held so a
/// switch cannot silently pick up an edit made since. Re-reading the configuration on each
/// switch would leave a person asking why it is using a model they did not choose.
pub async fn serve_catalog(
    listener: UnixListener,
    mut session: Session,
    backend: Option<turn::Backend>,
    catalog: crate::catalog::Catalog,
) -> Result<(), HostError> {
    // Told once, here, because this is the only place that knows both. A UI asking the
    // configuration for itself would report whatever is configured now rather than what this
    // daemon is actually talking to.
    session.set_choices(catalog.choices());
    session.set_model(backend.as_ref().map(|backend| axum_proto::ModelInfo {
        name: backend.model.qualified(),
        context_window: backend.model.context_window,
    }));
    let session = Arc::new(Mutex::new(session));
    // Turns run on the worker's own thread because a protocol lives in a Lua VM. A daemon
    // with no backend has no worker, and says so when a prompt arrives.
    //
    // Behind a lock because `/model` replaces it. Replaced rather than reconfigured: the
    // worker owns a VM built for one protocol, and handing a live VM a new one across a thread
    // boundary is a great deal of machinery to avoid rebuilding something that takes
    // milliseconds and happens by hand.
    // Shared with every connection, because a question raised by one turn has to be answerable
    // by whichever UI is attached when it arrives.
    let pending = Arc::new(crate::asking::Pending::new());
    // The asker publishes through the session's own broadcast handle rather than through the
    // lock: the thread that asks is the thread running the turn, which is usually the one
    // holding it.
    //
    // "Is anybody attached" is the subscriber count on that same channel, which is exactly the
    // question — a UI is attached precisely when it is listening.
    let approver: Arc<dyn axum_tools::approve::Approver> = {
        let events = session.lock().await.publisher();
        let watched = events.clone();
        Arc::new(crate::asking::Asker::new(
            Arc::clone(&pending),
            Box::new(move |event| {
                let _ = events.send(event);
            }),
            // Transient rather than journalled: a question is not part of the conversation, and
            // the UI tracks the highest cursor it has seen with a max, so zero disturbs nothing.
            Box::new(|| Cursor::ZERO),
            Box::new(move || watched.receiver_count() > 0),
        ))
    };
    let worker = Arc::new(tokio::sync::RwLock::new(
        backend
            .map(|backend| worker::Worker::gated(backend, Some(Arc::clone(&approver))))
            .map(Arc::new),
    ));
    let catalog = Arc::new(catalog);
    loop {
        let (stream, _) = listener.accept().await?;
        // The daemon serves one user. A connection from any other uid is refused rather than
        // authenticated, because there is no case where it should be served.
        match PeerCred::of(&stream) {
            Ok(cred) if cred.is_same_user() => {}
            _ => continue,
        }
        let session = Arc::clone(&session);
        let worker = Arc::clone(&worker);
        let catalog = Arc::clone(&catalog);
        let pending = Arc::clone(&pending);
        let approver = Arc::clone(&approver);
        tokio::spawn(async move {
            let _ = connection(stream, session, &worker, &catalog, &pending, &approver).await;
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
    approver: &Arc<dyn axum_tools::approve::Approver>,
) -> Result<(), HostError> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    let from = match reader.read::<UiCommand>().await? {
        UiCommand::Attach { from_cursor, .. } => from_cursor,
        // Anything before an attach is a peer that does not speak the protocol.
        _ => return Ok(()),
    };

    // Subscribe before reading state, so an entry committed between the two arrives on the
    // stream rather than falling into the gap.
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

    // Commands are read in their own task because `FrameReader::read` is not cancel-safe: it
    // takes a length and then a body, and a `select!` that drops it between the two leaves the
    // next read parsing body bytes as a length. Publishing an event used to do exactly that.
    let (commands, mut incoming) = tokio::sync::mpsc::channel::<UiCommand>(32);
    let mut reading = tokio::spawn(async move {
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
                    Some(UiCommand::SubmitPrompt { text }) => {
                        let held = worker.read().await.clone();
                        submit(&session, text, held, catalog).await?;
                    }
                    Some(UiCommand::DeclareNeeds) => {
                        let held = worker.read().await.clone();
                        if let Some(worker) = held {
                            // Spawned, because the declaration blocks on permission prompts and
                            // those are answered by commands read on this very loop.
                            let session = Arc::clone(&session);
                            tokio::spawn(async move { worker.declare(session).await });
                        }
                    }
                    Some(UiCommand::SetModel { name }) => {
                        if let Some(refusal) =
                            switch_model(&session, worker, catalog, approver, &name).await
                        {
                            // On the stream rather than in the transcript: the request was
                            // understood and declined, which is a fact about the UI's ask and
                            // not about the conversation.
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
                            switch_thinking(&session, worker, catalog, approver, &level).await
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
                        let cwd = catalog.cwd.display().to_string();
                        let dir = crate::paths::sessions_dir();
                        let refusal = match crate::paths::journal_for(&dir, &id) {
                            None => Some(format!("there is no session called {id:?}")),
                            Some(path) => session
                                .lock()
                                .await
                                .resume(&path, &cwd, seconds())
                                .err()
                                .map(|why| format!("{id} could not be opened: {why}")),
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
                            // What changes is what the provider is shown from now on.
                            let id = MessageId::new(format!("b{}", held.cursor().next().0));
                            held.commit(Entry::Branch { id, keeps })?;
                        }
                    }
                    // Handed straight to whoever is blocked on it. An id nobody is waiting on
                    // is dropped inside `answer`: the turn it belonged to is over.
                    Some(UiCommand::Permit { id, decision }) => {
                        pending.answer(&id, decision);
                    }
                    Some(UiCommand::Interrupt) => {
                        // The status is set here as well as by the turn: a stop the user asked for
                        // should show as stopped at once, not once the provider notices.
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
                    // Awaited in the branch body, not as a select arm: a cancelled write
                    // desyncs the stream the same way a cancelled read does.
                    Ok(event) => writer.write(&event).await?,
                    // A UI that fell behind reattaches with its cursor and replays; nothing is
                    // lost, because the journal is what actually holds the session.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            _ = &mut reading => break,
        }
    }

    reading.abort();
    Ok(())
}

/// Point the session at a different model, or say why not.
///
/// Returns `None` on success. The new worker is built before the old one is dropped, so a
/// switch that fails leaves the session able to carry on with what it had.
async fn switch_model(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    approver: &Arc<dyn axum_tools::approve::Approver>,
    name: &str,
) -> Option<String> {
    let Some(backend) = catalog.backend(name) else {
        return Some(catalog.unusable(name).unwrap_or_else(|| {
            let usable = catalog.usable();
            if usable.is_empty() {
                format!("there is no model called {name:?}, and none is configured")
            } else {
                format!(
                    "there is no model called {name:?}. Available: {}",
                    usable.join(", ")
                )
            }
        }));
    };

    let info = axum_proto::ModelInfo {
        name: backend.model.qualified(),
        context_window: backend.model.context_window,
    };
    // Gated, like the one it replaces. `Worker::start` is `gated(backend, None)` — a worker
    // nothing asks — so switching the model used to switch the permission model off with it,
    // and every tool for the rest of the session ran without being asked about.
    let fresh = Arc::new(worker::Worker::gated(backend, Some(Arc::clone(approver))));
    *worker.write().await = Some(fresh);
    {
        let mut held = session.lock().await;
        held.set_model(Some(info));
        // Announced so the footer changes now rather than after the next turn: the whole
        // point of switching is to see that it happened.
        held.announce_model();
        remember(catalog, held.model_name(), Some(held.thinking().to_owned()));
    }
    None
}

/// Write down what this directory is now using, so the next run starts with it.
///
/// A switch made in the UI is a decision somebody made in front of the thing. Forgetting it on
/// restart meant the only way to keep a choice was to stop making it in the UI and edit a file.
fn remember(catalog: &crate::catalog::Catalog, model: Option<String>, thinking: Option<String>) {
    let cwd = catalog.cwd.display().to_string();
    crate::remember::keep(&cwd, &crate::remember::Chosen { model, thinking });
}

/// Ask for more or less reasoning from here on, or say why not.
///
/// The worker is rebuilt for the same reason a model switch rebuilds it: the level rides on
/// every request, and the worker holds the backend the requests are built from.
async fn switch_thinking(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    approver: &Arc<dyn axum_tools::approve::Approver>,
    level: &str,
) -> Option<String> {
    let Ok(parsed) = serde_json::from_value::<axum_model::ThinkingLevel>(
        serde_json::Value::String(level.to_owned()),
    ) else {
        return Some(format!(
            "there is no thinking level called {level:?}. \
             Try off, minimal, low, medium, high or max."
        ));
    };

    // Rebuilt from the catalog rather than mutated in place, so the level is applied the same
    // way it would have been had the session started with it.
    let name = session.lock().await.model_name()?;
    let mut backend = catalog.backend(&name)?;
    backend.options.thinking = Some(parsed);
    // Gated, like the one it replaces. `Worker::start` is `gated(backend, None)` — a worker
    // nothing asks — so switching the model used to switch the permission model off with it,
    // and every tool for the rest of the session ran without being asked about.
    let fresh = Arc::new(worker::Worker::gated(backend, Some(Arc::clone(approver))));
    *worker.write().await = Some(fresh);
    let mut held = session.lock().await;
    held.set_thinking(level.to_owned());
    held.announce_model();
    remember(catalog, held.model_name(), Some(level.to_owned()));
    None
}

/// Why there is no model, in the terms of the config that produced the situation.
///
/// "No model is configured" was the whole of what this said, and it was wrong in the common
/// case: a model *is* configured, its provider's key is not set, and several other providers
/// are ready and waiting. Somebody whose environment holds an OpenRouter key reads that message
/// as OpenRouter being broken, because nothing in it mentions either fact.
fn no_model(catalog: &crate::catalog::Catalog) -> String {
    let chosen = catalog.chosen();
    let why = chosen.as_deref().and_then(|name| catalog.unusable(name));
    let ready = catalog.usable();

    let mut said = match (&chosen, &why) {
        (Some(name), Some(reason)) => format!("`{name}` cannot be used: {reason}."),
        (Some(name), None) => format!("`{name}` is not a model this build knows about."),
        (None, _) => "No model is configured.".to_owned(),
    };
    if ready.is_empty() {
        said.push_str(" Nothing else is ready either — set a provider key, or run `axum models` to see what each one needs.");
    } else {
        // A few, not all of them. Nine model ids is a wall of text in an error, and the point
        // is to get moving: `/model` is one keystroke from here and shows the rest.
        let shown = ready.len().min(3);
        let more = ready.len() - shown;
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        said.push_str(&format!(
            " Ready now: {}{tail}. Type `/model` to switch, or set `axum.model` in your config.",
            ready[..shown].join(", ")
        ));
    }
    said
}

/// Accept a prompt and run a turn.
///
/// The prompt is journalled before the provider is called, so an interrupted turn still shows
/// what was asked. Without a backend the refusal is a well-formed assistant entry rather than
/// an error out of band — the transcript stays uniform and the UI needs no second path.
async fn submit(
    session: &Arc<Mutex<Session>>,
    text: String,
    worker: Option<Arc<worker::Worker>>,
    catalog: &crate::catalog::Catalog,
) -> Result<(), HostError> {
    {
        let mut held = session.lock().await;
        // A stop belongs to the turn it interrupted. Left set, it would cancel the prompt typed
        // to replace the one the user just stopped.
        held.cancel().clear();
        let id = MessageId::new(format!("u{}", held.cursor().next().0));
        held.commit(Entry::User { id, text })?;
    }

    let Some(worker) = worker else {
        let mut held = session.lock().await;
        let id = MessageId::new(format!("a{}", held.cursor().next().0));
        held.commit(Entry::Assistant {
            id,
            text: String::new(),
            thinking: String::new(),
            stop_reason: Some(StopReason::Error),
            error: Some(no_model(catalog)),
            signatures: axum_proto::Signatures::default(),
            usage: axum_proto::Usage::default(),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(());
    };

    // Spawned, not awaited. This runs on the connection's own task, which is also the task
    // forwarding events to the attached UI: waiting here means nothing reaches the screen until
    // the turn is over, so a streaming response arrives all at once at the end.
    //
    // Overlapping turns are not a risk. The worker is one thread taking one job at a time, so
    // a second prompt queues behind the first exactly as it did when this awaited.
    let session = Arc::clone(session);
    tokio::spawn(async move { worker.run(session).await });
    Ok(())
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

/// Open the session a daemon should serve for `cwd`.
pub fn open_session(dir: &Path, cwd: &str, now: u64) -> Result<Session, JournalError> {
    let id = paths::session_id(now);
    let path = dir.join(format!("{id}.jsonl"));
    Session::open(&path, axum_proto::SessionId::new(id), cwd, now)
}

/// Seconds since the epoch, for stamping a journal that is being opened now.
fn seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
