//! The session: the journal, the socket, and the turns.
//!
//! Answers the same protocol the replay host answers, which is the whole test of M1: this
//! stands in for `magi fake-host` without a line of the UI moving.
//!
//! One session per process. It runs as a task inside the `magi` that shows it -- there is no
//! daemon -- so a session ends exactly when its window does. `UiCommand::Attach` already names
//! one, so growing to a registry would be a lookup rather than a protocol change.

pub mod asking;
pub mod cancel;
pub mod catalog;
pub mod compact;
pub mod context;
pub mod declaring;
pub mod paths;
pub mod remember;
pub mod scribe;
pub mod session;
pub mod system;
pub mod turn;
pub mod worker;

use magi_ipc::{FrameReader, FrameWriter, IpcError, PeerCred};
use magi_journal::JournalError;
use magi_proto::{
    AgentStatus, Cursor, Entry, ErrorClass, HarnessEvent, MessageId, StopReason, UiCommand,
};

use session::Session;
use std::path::Path;

use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

/// Anything that stops the session.
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
    // session is actually talking to.
    session.set_choices(catalog.choices());
    session.set_model(backend.as_ref().map(|backend| magi_proto::ModelInfo {
        name: backend.model.qualified(),
        context_window: backend.model.context_window,
    }));
    let session = Arc::new(Mutex::new(session));
    // Turns run on the worker's own thread because a protocol lives in a Lua VM. A session
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
    let approver: Arc<dyn magi_tools::approve::Approver> = {
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
    // No idle timer. There was one, and it counted the seconds since anything was attached: a
    // daemon outlived the UI that started it, so one per directory per afternoon was how
    // twenty-two of them ended up running, and this swept them.
    //
    // Nothing outlives its UI now — the session is a task in the process that shows it — so
    // there is nothing left to sweep, and keeping the timer would have been actively dangerous:
    // a UI whose connection hiccuped for long enough would have had its own session close the
    // socket underneath it, with no daemon left to restart and no way back.
    loop {
        let stream = listener.accept().await?.0;
        // A session serves one user. A connection from any other uid is refused rather than
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
    approver: &Arc<dyn magi_tools::approve::Approver>,
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
                    Some(UiCommand::SubmitPrompt { text, aside }) => {
                        let held = worker.read().await.clone();
                        submit(&session, Entry::User {
                            id: MessageId::new(format!("u{}", session.lock().await.cursor().next().0)),
                            text,
                            aside,
                        }, held, catalog).await?;
                    }
                    Some(UiCommand::Arrived { who, kin, sort, text }) => {
                        let arrived = Entry::From { who, kin, sort, text };
                        let wake = wants_answering(&arrived);
                        // Nothing another instance says interrupts a turn. A main with ten
                        // subagents would be answering the first one's question while the
                        // second, third and fourth arrive, and the moment it is mid-thought is
                        // the worst one to hand it somebody else's. What arrives now is dealt
                        // with when the turn it arrived during is over -- see `after`.
                        if !session.lock().await.idle() {
                            session.lock().await.hold(arrived);
                            continue;
                        }
                        let held = worker.read().await.clone();
                        if wake {
                            // A turn, the same way a prompt starts one. This is what makes a
                            // message a *message*: without it the entry landed in the transcript
                            // and nothing read it, so an instance could be asked a question and
                            // would sit there until somebody typed at it.
                            submit(&session, arrived, held, catalog).await?;
                        } else {
                            // Committed and no more. A note is something to have seen by the
                            // time you next answer, not a reason to start answering.
                            session.lock().await.commit(arrived)?;
                        }
                    }
                    Some(UiCommand::TakeGrants { grants }) => {
                        let held = worker.read().await.clone();
                        if let Some(worker) = held {
                            // Queued like a turn, so one already running finishes under the
                            // permissions it started with. A tool that had checked and been
                            // refused should not find the answer different halfway through.
                            worker.take_on(Arc::clone(&session), grants).await;
                        }
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
    approver: &Arc<dyn magi_tools::approve::Approver>,
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

    let info = magi_proto::ModelInfo {
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
    approver: &Arc<dyn magi_tools::approve::Approver>,
    level: &str,
) -> Option<String> {
    let Ok(parsed) = serde_json::from_value::<magi_model::ThinkingLevel>(
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
/// Public because the UI says the same thing at attach, and said it worse: a fixed "No model is
/// configured" on screen while the daemon, on the first prompt, gave the real reason. Two answers
/// to one question, and the one you met first was the wrong one.
///
/// "No model is configured" was the whole of what this said, and it was wrong in the common
/// case: a model *is* configured, its provider's key is not set, and several other providers
/// are ready and waiting. Somebody whose environment holds an OpenRouter key reads that message
/// as OpenRouter being broken, because nothing in it mentions either fact.
#[must_use]
pub fn no_model(catalog: &crate::catalog::Catalog) -> String {
    let chosen = catalog.chosen();
    let why = chosen.as_deref().and_then(|name| catalog.unusable(name));
    let ready = catalog.usable();

    let mut said = match (&chosen, &why) {
        // The reason already opens with the name, so prefixing it printed the model twice in
        // one sentence.
        (Some(_), Some(reason)) => format!("{reason}."),
        (Some(name), None) => format!("`{name}` is not a model this build knows about."),
        (None, _) => "No model is configured.".to_owned(),
    };
    if ready.is_empty() {
        said.push_str(" Nothing else is ready either — set a provider key, or run `magi models` to see what each one needs.");
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
            " Ready now: {}{tail}. Type `/model` to switch, or set `magi.model` in your config.",
            ready[..shown].join(", ")
        ));
    }
    said
}

/// Journal what opened a turn, and run it.
///
/// `opening` is what was said and by whom: a prompt somebody typed, or a message another
/// instance sent. Both start a turn the same way and for the same reason — something addressed
/// to this session arrived and wants an answer — so they are one path rather than two that
/// would drift.
///
/// It is journalled before the provider is called, so an interrupted turn still shows what was
/// asked. Without a backend the refusal is a well-formed assistant entry rather than an error
/// out of band — the transcript stays uniform and the UI needs no second path.
async fn submit(
    session: &Arc<Mutex<Session>>,
    opening: Entry,
    worker: Option<Arc<worker::Worker>>,
    catalog: &crate::catalog::Catalog,
) -> Result<(), HostError> {
    {
        let mut held = session.lock().await;
        // A stop belongs to the turn it interrupted. Left set, it would cancel the prompt typed
        // to replace the one the user just stopped.
        held.cancel().clear();
        held.commit(opening)?;
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
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
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
    tokio::spawn(async move { after(session, worker).await });
    Ok(())
}

/// Run a turn, then deal with whatever arrived while it was running.
///
/// The other half of the waiting room. An arrival during a turn is held rather than delivered
/// — see [`session::Session::waiting`] — and this is where it comes back out: the turn ends,
/// the messages are committed in the order they came, and if any of them wanted an answer, one
/// more turn runs to give it.
///
/// A loop, because more can arrive during *that* turn. It ends when a turn finishes with an
/// empty waiting room, which is the ordinary case: a session with nobody talking to it does one
/// pass and stops.
async fn after(session: Arc<Mutex<Session>>, worker: Arc<worker::Worker>) {
    loop {
        worker.run(Arc::clone(&session)).await;

        let arrived = session.lock().await.release();
        if arrived.is_empty() {
            return;
        }
        // Committed together and answered once. Ten subagents reporting during one turn is ten
        // things to read and one turn to read them in — waking once per message would spend a
        // turn on each and let the last of them arrive during the answer to the first.
        let mut answer = false;
        {
            let mut held = session.lock().await;
            for entry in arrived {
                answer |= wants_answering(&entry);
                if held.commit(entry).is_err() {
                    return;
                }
            }
        }
        if !answer {
            return;
        }
    }
}

/// Whether a message that arrived is one the session should answer rather than merely have read.
///
/// The sender chose, by which verb they used. A note is something to have seen by the time you
/// next reply; a question, an answer to one you asked, a call for help, work handed to you, or a
/// report that something has gone wrong is not.
///
/// **Not the same question as "may this interrupt".** A running turn is interrupted only by
/// `attention` and `trouble`, and the layer decides that. This is the other one: an *idle*
/// session, and whether what just arrived is a reason to think. Answering it too narrowly is
/// silent — nothing fails, the entry is in the transcript, and the session simply sits there.
///
/// That is what left `ask` a one-way trip. `ask` sends a `question`, which woke the receiver;
/// `reply` sends an `answer`, which was not on this list, so the reply reached the asker's
/// transcript and nothing ran. Two agents got exactly one exchange and then stopped, and the
/// only symptom was silence.
///
/// `claim` and `release` stay off it on purpose: they say what somebody else is doing, and a
/// session that started a turn over every one of them would spend the day on bookkeeping.
///
/// **The one place that decides.** It was two: the UI worked it out from its own `Sort` enum and
/// put the answer on the wire, and this worked it out again from the string. Two rules for one
/// question, in two vocabularies, and nothing would have failed when they drifted — a sort added
/// to one would just quietly stop waking anybody.
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

/// Open the session a daemon should serve for `cwd`.
pub fn open_session(dir: &Path, cwd: &str, now: u64, whose: &str) -> Result<Session, JournalError> {
    let id = paths::session_id(now, whose);
    let path = dir.join(format!("{id}.jsonl"));
    Session::open(&path, magi_proto::SessionId::new(id), cwd, now)
}

/// Seconds since the epoch, for stamping a journal that is being opened now.
fn seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod no_model_tests {
    use crate::catalog::Catalog;

    /// A catalog naming a model whose provider has no credential.
    fn wanting(name: &str) -> Catalog {
        let providers =
            serde_json::from_value::<Vec<magi_provider::provider::Provider>>(serde_json::json!([{
                "id": "paid", "name": "Paid Co", "api": "openai-completions",
                "base_url": "https://paid.test/v1",
                "auth": { "kind": "api-key", "vars": ["MAGI_TEST_NOT_SET"] },
                "models": [{ "id": "x", "name": "X", "context_window": 1000, "max_tokens": 100 }]
            }]))
            .expect("providers");
        Catalog {
            chosen: Some(name.to_owned()),
            providers,
            ..Catalog::empty()
        }
    }

    #[test]
    fn a_configured_model_with_no_key_is_not_called_unconfigured() {
        // What the UI met at attach was "No model is configured", on a machine whose
        // `magi.model` was set and whose key merely was not. Two different problems, one
        // sentence, and the sentence sent people to the wrong one.
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
