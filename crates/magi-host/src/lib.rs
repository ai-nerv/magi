//! The session: the journal, the socket, and the turns.
//!
//! Answers the same protocol the replay host answers, which is the whole test of M1: this
//! stands in for `magi fake-host` without a line of the UI moving.
//!
//! One session per process. It runs as a task inside the `magi` that shows it -- there is no
//! daemon -- so a session ends exactly when its window does. `UiCommand::Attach` already names
//! one, so growing to a registry would be a lookup rather than a protocol change.

pub mod asking;
pub mod broker;
pub mod cancel;
pub mod catalog;
pub mod compact;
pub mod context;
pub mod declaring;
pub mod driving;
pub mod holder;
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
    AgentStatus, Cursor, Entry, ErrorClass, HarnessEvent, MessageId, SessionId, StopReason,
    UiCommand,
};

use session::Session;
use std::path::Path;

use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

/// What [`drain`] needs to reach, set once the session is serving.
///
/// A process-global because the process *is* one session — see this module's own note — so
/// there is nothing to disambiguate. It exists because a turn's flush runs on a spawned task
/// and a process can exit before that task is scheduled: `magi -p` prints its answer the moment
/// the assistant entry settles, which is earlier than the turn boundary the flush waits for.
type Draining = (
    Arc<Mutex<Session>>,
    Arc<Mutex<Option<crate::scribe::Scribe>>>,
);
static DRAINING: std::sync::OnceLock<Draining> = std::sync::OnceLock::new();

/// Hand over anything a turn settled that has not reached balthasar yet.
///
/// Called on the way out, after the last turn and before the socket goes. Does nothing when no
/// session is serving or no balthasar was found.
pub async fn drain() {
    let Some((session, scribe)) = DRAINING.get() else {
        return;
    };
    let _ = crate::scribe::flush(session, &mut *scribe.lock().await).await;
}

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
    session: Session,
    backend: Option<turn::Backend>,
    catalog: crate::catalog::Catalog,
) -> Result<(), HostError> {
    serve_on(listener, session, backend, catalog, None).await
}

/// The same, told which balthasar to record into.
///
/// A path rather than a search. magi starts a balthasar of its own and must talk to *that* one:
/// the newest socket in the directory is a neighbour's as often as not, and two windows writing
/// each other's transcripts is the failure this naming exists to prevent.
pub async fn serve_on(
    listener: UnixListener,
    mut session: Session,
    backend: Option<turn::Backend>,
    catalog: crate::catalog::Catalog,
    balthasar: Option<std::path::PathBuf>,
) -> Result<(), HostError> {
    // Told once, here, because this is the only place that knows both. A UI asking the
    // configuration for itself would report whatever is configured now rather than what this
    // session is actually talking to.
    session.set_choices(catalog.choices());
    session.set_model(backend.as_ref().map(|backend| magi_proto::ModelInfo {
        name: backend.model.clone(),
        context_window: backend.context_window.unwrap_or(0),
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
    // Dialled once, here, and `None` when balthasar is not running. Absent is the ordinary case
    // while the journal is still the copy of record: nothing is registered, nothing is written,
    // and the session behaves exactly as it did before balthasar existed.
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
    // The asker publishes through the session's own broadcast handle rather than through the
    // lock: the thread that asks is the thread running the turn, which is usually the one
    // holding it.
    //
    // "Is anybody attached" is the subscriber count on that same channel, which is exactly the
    // question — a UI is attached precisely when it is listening.
    // One asker, two traits. It answers both kinds of question — a permission and anything else
    // a tool wants to put to the person — and both travel the same way: out as an event, back on
    // a channel. Two askers would be two ids counting from zero into one map.
    //
    // Built before the asker, because the asker draws its permission prompt on one: the prompt is
    // casper's now, and magi keeps only the deciding.
    let holding = Arc::new(crate::holder::Holding::new());
    let holds: Arc<dyn magi_tools::holding::Holds> = {
        let events = session.lock().await.publisher();
        let watched = events.clone();
        Arc::new(crate::holder::Holder::new(
            Arc::clone(&holding),
            Box::new(move |event| {
                let _ = events.send(event);
            }),
            Box::new(move || watched.receiver_count() > 0),
            magi_tools::casper::CASPER,
        ))
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
                // Transient rather than journalled: a question is not part of the conversation,
                // and the UI tracks the highest cursor it has seen with a max, so zero disturbs
                // nothing.
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
                )
            })
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
    // **Whether anybody here can draw rows a tool asks for.** `magi -p` cannot, and a session that
    // reserved rows for it would hold the turn open until the surface timed out, waiting on a
    // keypress that was never coming.
    //
    // Held for the life of the connection, so a UI that goes away takes its screen with it rather
    // than leaving the session believing there is still one.
    let _drawing = crate::holder::Drawing::attach(&person.surfaces, draws);

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
                        }, held, catalog, scribe).await?;
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
                            submit(&session, arrived, held, catalog, scribe).await?;
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
                            switch_model(&session, worker, catalog, person, &name).await
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
                            switch_thinking(&session, worker, catalog, person, &level).await
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
                        // balthasar first, and by replay rather than by file: it is the store,
                        // so a journal still on disk is either absent or behind.
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
                            _ => match crate::paths::journal_for(&dir, &id) {
                                None => Some(format!("there is no session called {id:?}")),
                                Some(path) => session
                                    .lock()
                                    .await
                                    .resume(&path, &cwd, seconds())
                                    .err()
                                    .map(|why| format!("{id} could not be opened: {why}")),
                            },
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
                    // The same, for a question a tool asked in its own words.
                    Some(UiCommand::Answered { id, choice }) => {
                        pending.chose(&id, choice);
                    }
                    // How wide the screen is. The session has no terminal, so this is the only
                    // way anything drawing on one can know what it has.
                    Some(UiCommand::Sized { cols, holds }) => person.surfaces.sized(cols, holds),
                    // A key aimed at rows a tool is holding. Not interpreted on the way through:
                    // what `j` means is the tenant's business, and a harness that decided would
                    // be back to owning the thing it just handed over.
                    Some(UiCommand::Keyed { id, key, state }) => {
                        person.surfaces.keyed(&id, key, state);
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
#[path = "switching.rs"]
mod switching;
use switching::{switch_model, switch_thinking};
// Re-exported: it answers "why is nothing configured", which is a question the UI asks at attach
// and not something about switching a model.
pub use switching::no_model;

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
    scribe: &Arc<Mutex<Option<crate::scribe::Scribe>>>,
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
    let scribe = Arc::clone(scribe);
    tokio::spawn(async move { after(session, worker, scribe).await });
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
async fn after(
    session: Arc<Mutex<Session>>,
    worker: Arc<worker::Worker>,
    scribe: Arc<Mutex<Option<crate::scribe::Scribe>>>,
) {
    loop {
        worker.run(Arc::clone(&session)).await;

        // The turn boundary, which is where durability is owed. Amendments during streaming are
        // coalesced by cursor in the session, so a message written a hundred times on the way
        // through goes over once, as it finally stood.
        if let Err(fault) = crate::scribe::flush(&session, &mut *scribe.lock().await).await {
            // Said once, in the transcript, rather than swallowed. A session whose transcript
            // stopped being recorded must not look like one that is fine, and while magi's own
            // journal is still the copy of record this costs memory rather than the session.
            let mut held = session.lock().await;
            let id = MessageId::new(format!("n{}", held.cursor().next().0));
            let _ = held.commit(Entry::Assistant {
                id,
                text: String::new(),
                thinking: String::new(),
                stop_reason: Some(StopReason::Error),
                error: Some(format!("this turn was not recorded: {fault}")),
                signatures: magi_proto::Signatures::default(),
                usage: magi_proto::Usage::default(),
            });
        }

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
