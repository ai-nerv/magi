//! Listening, so another instance can reach this one.
//!
//! The other half of the mirror. Every axon binds this, so being asked and asking are the same
//! session in two directions rather than a supervisor and a worker with different vocabularies.
//!
//! Nothing here decides anything: it frames bytes, works out where the caller sits in the tree,
//! and hands both to [`super::answering::answer`], which is where the permission check and the
//! vocabulary live. What this file owns is the parts that only go wrong under a real socket — a
//! connection that serves one call and dies, a peer that reads slowly, a socket left behind by
//! a crash.

use super::answering::{About, Then, answer};
use super::policy::Whom;
use super::wire::{Call, Message, Reply};
use super::{inside, whom};
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
        // Belt and braces: the path is built from a project name, and a project name is the
        // working directory's, which can be anything.
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
        // Another user's process gets nothing at all, whatever it says about itself. Everything
        // finer than that is a relation, and a relation is read off the directory.
        if !ours(&stream) {
            continue;
        }
        // Refused rather than queued: a caller told "busy" now can ask again, while one held in
        // an accept queue waits on a session that may be blocked for a whole turn.
        let Ok(permit) = std::sync::Arc::clone(&held).try_acquire_owned() else {
            continue;
        };
        let serving = Serving {
            about: serving.about.clone(),
            arrived: serving.arrived.clone(),
            stopped: serving.stopped.clone(),
        };
        tokio::spawn(async move {
            let _permit = permit;
            let _ = talk(stream, serving).await;
        });
    }
}

/// One connection, for as many calls as it cares to make.
///
/// It keeps serving after replying. Closing after one is a tempting simplification and it means
/// a client that holds a connection — the obvious way to write one — dies on its *second* call
/// with a broken pipe.
async fn talk(stream: tokio::net::UnixStream, serving: Serving) -> std::io::Result<()> {
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
        // Looked up per call rather than once per connection: a session that forks a child
        // mid-conversation has a new child, and a connection held open would go on answering
        // with the tree as it stood when it was opened.
        let caller = placed(call.from.as_deref(), &about);
        let (reply, then) = answer(&call, &about, caller.as_ref());
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

/// Whether the far end is this user at all.
///
/// Taken from `SO_PEERCRED`, never from anything the caller sent — the whole point of asking the
/// kernel is that the answer is not the caller's to choose. It is also the *only* thing the
/// kernel can settle: every session in a project runs as one user, so which of them is calling
/// is a question `SO_PEERCRED` cannot answer, and that one is answered by the directory and by
/// the secret instead.
fn ours(stream: &tokio::net::UnixStream) -> bool {
    axon_ipc::PeerCred::of(stream).is_ok_and(|cred| cred.is_same_user())
}

/// Where a caller sits in the tree, from the name it gave.
///
/// The name is the caller's; the *place* is not. Once the name is read, the parent that decides
/// what it may do is looked up in the project directory, so a session cannot claim to be
/// somebody's child and be believed.
///
/// A name from another project resolves to a stranger rather than to nothing, so the policy
/// refuses it as `elsewhere` and the refusal can say which wall it met. `None` is kept for a
/// caller that said nothing at all, which is a different mistake and gets a different answer.
fn placed(from: Option<&str>, about: &About) -> Option<Whom> {
    let (project, id) = from?.split_once('/')?;
    if project.is_empty() || id.is_empty() {
        return None;
    }
    if project != about.me.project {
        return Some(Whom {
            project: project.to_owned(),
            id: id.to_owned(),
            parent: None,
        });
    }
    Some(whom(project, id))
}
