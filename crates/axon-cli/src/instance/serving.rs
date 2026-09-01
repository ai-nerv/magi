//! Listening, so another instance can reach this one.
//!
//! The other half of the mirror. Every axon binds this, so being asked and asking are the same
//! session in two directions rather than a supervisor and a worker with different vocabularies.
//!
//! Nothing here decides anything: it frames bytes and hands the call to
//! [`super::answering::answer`], which is where the permission check and the vocabulary live.
//! What this file owns is the parts that only go wrong under a real socket — a connection that
//! serves one call and dies, a peer that reads slowly, a socket left behind by a crash.

use super::answering::{About, Then, answer};
use super::wire::{Call, Message, Reply};
use super::{Kind, inside};
use std::path::Path;
use tokio::sync::mpsc;

/// How many callers may be connected at once.
///
/// Bounded because a socket in the runtime directory is reachable by anything running as this
/// user, and an unbounded accept loop is a file-descriptor exhaustion away from taking the UI
/// with it.
const CALLERS: usize = 8;

/// How long a connection may sit idle before it is dropped.
const IDLE: std::time::Duration = std::time::Duration::from_secs(300);

/// What the socket needs from the session, and what it sends back to it.
pub struct Serving {
    /// Told what this instance is doing, whenever a call needs to know.
    pub about: tokio::sync::watch::Receiver<About>,
    /// Messages that arrived, on their way to the inbox.
    pub arrived: mpsc::Sender<Message>,
    /// Somebody with the right to stop this instance did.
    pub stopped: mpsc::Sender<()>,
}

/// Listen on `path` until the process ends.
///
/// A stale socket is cleared first: a path nothing answers makes `bind` fail with `EADDRINUSE`
/// even though the process that made it is long gone, and the alternative is a session that
/// cannot be reached because a previous one crashed.
pub async fn serve(path: &Path, serving: Serving) -> std::io::Result<()> {
    if !inside(path) {
        // Belt and braces: the path is built from a name, and a name is not a promise.
        return Err(std::io::Error::other("that is not an instance socket"));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = axon_ipc::bind(path)
        .await
        .map_err(|why| std::io::Error::other(why.to_string()))?;
    let held = std::sync::Arc::new(tokio::sync::Semaphore::new(CALLERS));

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        // Refused rather than queued: a caller told "busy" now can ask again, while one held in
        // an accept queue waits on a session that may be blocked for a whole turn.
        let Ok(permit) = std::sync::Arc::clone(&held).try_acquire_owned() else {
            continue;
        };
        // Whoever is at the other end decides what they may do. Taken from the kernel rather
        // than from anything the caller says about itself.
        let kind = whose(&stream);
        let serving = Serving {
            about: serving.about.clone(),
            arrived: serving.arrived.clone(),
            stopped: serving.stopped.clone(),
        };
        tokio::spawn(async move {
            let _permit = permit;
            let _ = talk(stream, serving, kind).await;
        });
    }
}

/// One connection, for as many calls as it cares to make.
///
/// It keeps serving after replying. Closing after one is a tempting simplification and it means
/// a client that holds a connection — the obvious way to write one — dies on its *second* call
/// with a broken pipe.
async fn talk(stream: tokio::net::UnixStream, serving: Serving, kind: Kind) -> std::io::Result<()> {
    let (read, write) = stream.into_split();
    let mut reader = axon_ipc::FrameReader::new(read);
    let mut writer = axon_ipc::FrameWriter::new(write);

    loop {
        let call: Call = match tokio::time::timeout(IDLE, reader.read()).await {
            Ok(Ok(call)) => call,
            // A malformed frame is answered rather than dropped, so the caller sees axon's
            // error instead of a transport one. Then the connection ends: a stream that has
            // lost its framing cannot be resynchronised.
            Ok(Err(why)) => {
                let _ = writer.write(&Reply::refused(why.to_string())).await;
                return Ok(());
            }
            Err(_) => return Ok(()),
        };
        let about = serving.about.borrow().clone();
        let (reply, then) = answer(&call, &about, kind);
        writer
            .write(&reply)
            .await
            .map_err(|why| std::io::Error::other(why.to_string()))?;
        match then {
            Then::Nothing => {}
            Then::Keep(message) => {
                let _ = serving.arrived.send(message).await;
            }
            Then::Stop => {
                let _ = serving.stopped.send(()).await;
                return Ok(());
            }
        }
    }
}

/// What the far end is allowed to be.
///
/// Only a process running as this user is a fork; anything else is a peer and may ask and
/// nothing more. Taken from `SO_PEERCRED`, never from a number the caller sent — the whole
/// point of asking the kernel is that the answer is not the caller's to choose.
///
/// This is the floor, not the finished rule. Once forks are actually started, the parent will
/// know which pids it spawned and this becomes "a fork is one I started"; until then the
/// permission model is enforced and the set it is enforced over is generous.
fn whose(stream: &tokio::net::UnixStream) -> Kind {
    axon_ipc::PeerCred::of(stream)
        .ok()
        .filter(|cred| cred.is_same_user())
        .map_or(Kind::Peer, |_| Kind::Fork)
}
