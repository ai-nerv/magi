//! The axum daemon.
//!
//! Owns the session, the journal, and the socket. Answers the same protocol the replay host
//! answers, which is the whole test of M1: `axum host` stands in for `axum fake-host` without
//! a line of the UI moving.
//!
//! One session per daemon, for now. `UiCommand::Attach` already names one, so growing to a
//! registry is a lookup rather than a protocol change.

pub mod paths;
pub mod session;

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
pub async fn serve(listener: UnixListener, session: Session) -> Result<(), HostError> {
    let session = Arc::new(Mutex::new(session));
    loop {
        let (stream, _) = listener.accept().await?;
        // The daemon serves one user. A connection from any other uid is refused rather than
        // authenticated, because there is no case where it should be served.
        match PeerCred::of(&stream) {
            Ok(cred) if cred.is_same_user() => {}
            _ => continue,
        }
        let session = Arc::clone(&session);
        tokio::spawn(async move {
            let _ = connection(stream, session).await;
        });
    }
}

/// One attached UI.
async fn connection(stream: UnixStream, session: Arc<Mutex<Session>>) -> Result<(), HostError> {
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
                    Some(UiCommand::SubmitPrompt { text }) => submit(&session, text).await?,
                    Some(UiCommand::Interrupt) => {
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

/// Accept a prompt.
///
/// M1 has no provider, so the turn is the user's message and an honest refusal. M2 replaces
/// the second half; the journalling and publication either side of it do not change.
async fn submit(session: &Arc<Mutex<Session>>, text: String) -> Result<(), HostError> {
    let mut session = session.lock().await;

    let user_id = MessageId::new(format!("u{}", session.cursor().next().0));
    session.commit(Entry::User { id: user_id, text })?;

    let reply_id = MessageId::new(format!("a{}", session.cursor().next().0));
    session.commit(Entry::Assistant {
        id: reply_id,
        text: String::new(),
        thinking: String::new(),
        stop_reason: Some(StopReason::Error),
        error: Some("no provider is configured yet — that lands in M2".into()),
    })?;
    session.set_status(AgentStatus::Idle);
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
