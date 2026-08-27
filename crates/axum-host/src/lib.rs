//! The axum daemon.
//!
//! Owns the session, the journal, and the socket. Answers the same protocol the replay host
//! answers, which is the whole test of M1: `axum host` stands in for `axum fake-host` without
//! a line of the UI moving.
//!
//! One session per daemon, for now. `UiCommand::Attach` already names one, so growing to a
//! registry is a lookup rather than a protocol change.

pub mod cancel;
pub mod paths;
pub mod session;
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
    let session = Arc::new(Mutex::new(session));
    // Turns run on the worker's own thread because a protocol lives in a Lua VM. A daemon
    // with no backend has no worker, and says so when a prompt arrives.
    let worker = backend.map(worker::Worker::start).map(Arc::new);
    loop {
        let (stream, _) = listener.accept().await?;
        // The daemon serves one user. A connection from any other uid is refused rather than
        // authenticated, because there is no case where it should be served.
        match PeerCred::of(&stream) {
            Ok(cred) if cred.is_same_user() => {}
            _ => continue,
        }
        let session = Arc::clone(&session);
        let worker = worker.clone();
        tokio::spawn(async move {
            let _ = connection(stream, session, worker).await;
        });
    }
}

/// One attached UI.
async fn connection(
    stream: UnixStream,
    session: Arc<Mutex<Session>>,
    worker: Option<Arc<worker::Worker>>,
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
                        submit(&session, text, worker.clone()).await?;
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

/// Accept a prompt and run a turn.
///
/// The prompt is journalled before the provider is called, so an interrupted turn still shows
/// what was asked. Without a backend the refusal is a well-formed assistant entry rather than
/// an error out of band — the transcript stays uniform and the UI needs no second path.
async fn submit(
    session: &Arc<Mutex<Session>>,
    text: String,
    worker: Option<Arc<worker::Worker>>,
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
            error: Some(
                "no model is configured. Set a provider key, or choose one with `axum.model` \
                 in your config; `axum models` lists what is available."
                    .into(),
            ),
            signatures: axum_proto::Signatures::default(),
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
