//! Listening, so another instance can reach this one.
//!
//! The other half of the mirror. Every axon binds this, so being asked and asking are the same
//! session in two directions rather than a supervisor and a worker with different vocabularies.
//!
//! Nothing here decides anything: it frames bytes, works out where the caller sits in the tree,
//! and hands both to [`crate::answering::answer`], which is where the permission check and the
//! vocabulary live. What this file owns is the parts that only go wrong under a real socket — a
//! connection that serves one call and dies, a peer that reads slowly, a socket left behind by
//! a crash.

use crate::answering::{About, Then, answer};
use crate::directory::{inside, whom};
use crate::framing;
use crate::policy::Whom;
use crate::wire::{Call, Message, Reply};
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
    let listener = bind(path).await?;
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
    let (mut reader, mut writer) = stream.into_split();

    loop {
        let call: Call = match tokio::time::timeout(IDLE, framing::read(&mut reader)).await {
            Ok(Ok(call)) => call,
            // A malformed frame is answered rather than dropped, so the caller sees axon's
            // error instead of a transport one. Then the connection ends: a stream that has
            // lost its framing cannot be resynchronised.
            Ok(Err(why)) => {
                let _ = framing::write(&mut writer, &Reply::refused(why.to_string())).await;
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
        framing::write(&mut writer, &reply).await?;
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
    stream
        .peer_cred()
        .is_ok_and(|cred| cred.uid() == rustix::process::getuid().as_raw())
}

/// Listen at `path`, clearing what a crash left behind.
///
/// Twenty lines rather than a dependency. A socket file outlives the process that made it, so
/// `bind` fails with `EADDRINUSE` on a path nothing has answered since a machine slept — and
/// the alternative to clearing it is a session that cannot be reached because a previous one
/// died badly.
///
/// **Connected to before removed.** A path that answers belongs to somebody: unlinking it would
/// take a running session's socket out from under it, and the two would then both think they
/// were reachable while only one of them was.
async fn bind(path: &Path) -> std::io::Result<tokio::net::UnixListener> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::metadata(path).await.is_ok()
        && tokio::net::UnixStream::connect(path).await.is_err()
    {
        tokio::fs::remove_file(path).await?;
    }
    tokio::net::UnixListener::bind(path)
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
    // Parsed as a whole name, never split at the first slash: `axon/review/iota-mu` cut that way
    // gives a session called `review/iota-mu`, which is nobody, and every relation it has is
    // wrong. The role in it is dropped here on purpose — it is the caller's own description of
    // itself and nothing is decided by it.
    let them = crate::identity::Identity::read(from?)?;
    if them.project != about.me.project {
        return Some(Whom {
            project: them.project,
            id: them.id,
            parent: None,
        });
    }
    Some(whom(&them.project, &them.id))
}

/// Two instances, one socket, and a message that actually arrives.
///
/// Everything else about this surface is decided without a socket, on purpose — the walls, the
/// vocabulary, the refusals. This is the one thing that cannot be: that the client half and the
/// server half agree about what goes on the wire.
///
/// They did not, once. The socket was framed with `axon_ipc` — CBOR inside an envelope carrying
/// a protocol version — and documented as the family's four-byte length and a JSON body. Both
/// ends of axon agreed with each other perfectly, every test passed, and no sibling tool could
/// have said a word to it. That failure is invisible from inside the program that owns it,
/// which is why these bind a real socket.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::asking::Held;
    use crate::identity::Identity;
    use crate::wire::Sort;
    use std::time::Duration;

    /// A project nothing else is using.
    ///
    /// The runtime directory is shared with whatever axons the person has open, and the whole
    /// point of a project directory is that one project cannot see another's.
    /// One per test, because they run at once and each clears up after itself. Sharing a
    // project directory made every test tear down the sockets the others were using.
    fn project(tag: &str) -> String {
        format!("axon-test-{}-{tag}", std::process::id())
    }

    pub(super) fn named(tag: &str, id: &str) -> Identity {
        Identity {
            project: project(tag),
            role: "main".to_owned(),
            id: id.to_owned(),
        }
    }

    pub(super) fn tidy(tag: &str) {
        let _ = std::fs::remove_dir_all(crate::directory::home(&project(tag)));
    }

    /// What a bound session hands back, held for as long as the test needs it.
    pub(super) struct Bound {
        arrived: mpsc::Receiver<Message>,
        /// Dropping this closes the channel the socket reads the session's state from, and
        /// every call would then answer with whatever was left behind.
        _about: tokio::sync::watch::Sender<About>,
        _stopped: mpsc::Receiver<()>,
    }

    pub(super) async fn listening(me: &Identity) -> Bound {
        let (about, about_rx) = tokio::sync::watch::channel(About {
            me: me.clone(),
            parent: None,
            token: None,
            busy: false,
            working_for: 0,
            inbox: Vec::new(),
        });
        let (arrived_tx, arrived) = mpsc::channel(8);
        let (stopped_tx, stopped) = mpsc::channel(1);
        let at = crate::directory::listening_at(me);
        tokio::spawn(async move {
            let _ = serve(
                &at,
                Serving {
                    about: about_rx,
                    arrived: arrived_tx,
                    stopped: stopped_tx,
                },
            )
            .await;
        });
        // The bind is a few awaits away and the first dial would otherwise race it.
        let at = crate::directory::listening_at(me);
        for _ in 0..100 {
            if Held::at(&at, me).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Bound {
            arrived,
            _about: about,
            _stopped: stopped,
        }
    }

    #[tokio::test]
    async fn a_message_sent_by_one_instance_arrives_at_another() {
        let them = named("arrives", "beta-nu");
        let me = named("arrives", "alpha-rho");
        let mut bound = listening(&them).await;

        // From a blocking thread, because that is where it happens for real: the client half is
        // a tool peer, which has no runtime and wants none.
        let (they, i) = (them.clone(), me.clone());
        let sent = tokio::task::spawn_blocking(move || {
            let mut held = Held::to(&they, &i).expect("it is listening");
            held.call(
                "tell",
                vec![
                    serde_json::json!("the parser is done"),
                    serde_json::json!("attention"),
                    serde_json::json!(null),
                ],
            )
            .expect("answered")
        })
        .await
        .expect("the thread finished");

        assert!(sent.ok, "{sent:?}");
        let message = tokio::time::timeout(Duration::from_secs(5), bound.arrived.recv())
            .await
            .expect("it arrived")
            .expect("the channel is open");
        assert_eq!(message.text, "the parser is done");
        // Never from an argument. A message that could name its own sender is one anybody can
        // forge into anybody's inbox.
        assert_eq!(message.from, me.full());
        assert_eq!(message.sort, Sort::Attention);
        assert!(message.sort.interrupts());
        tidy("arrives");
    }

    #[tokio::test]
    async fn one_connection_serves_several_calls() {
        // The family's guidance names this one: a client that holds a connection is the obvious
        // way to write one, and against a server that closes after replying it dies on its
        // *second* call with a broken pipe.
        let them = named("held", "gamma-xi");
        let me = named("held", "delta-pi");
        let _bound = listening(&them).await;

        let (they, i) = (them.clone(), me.clone());
        let answers = tokio::task::spawn_blocking(move || {
            let mut held = Held::to(&they, &i).expect("it is listening");
            ["verbs", "identity", "status", "identity"]
                .into_iter()
                .map(|verb| held.call(verb, Vec::new()).expect("answered"))
                .collect::<Vec<_>>()
        })
        .await
        .expect("the thread finished");

        for (at, reply) in answers.iter().enumerate() {
            assert!(reply.ok, "call {at} failed: {reply:?}");
            assert_eq!(reply.n, reply.result.len(), "call {at}");
        }
        assert_eq!(answers[1].result[0]["id"], them.id);
        tidy("held");
    }

    #[tokio::test]
    async fn a_main_refuses_a_stop_from_another_main_at_the_wall() {
        // Two gates stand between a caller and a stop, and this is the outer one: nobody
        // started this session, so no relation makes the caller its parent and the secret is
        // never even looked at.
        let them = named("wall", "epsilon-tau");
        let me = named("wall", "zeta-nu");
        let _bound = listening(&them).await;

        let (they, i) = (them.clone(), me.clone());
        let reply = tokio::task::spawn_blocking(move || {
            let mut held = Held::to(&they, &i).expect("it is listening");
            held.call_with("stop", Vec::new(), "guessed")
                .expect("answered")
        })
        .await
        .expect("the thread finished");

        assert!(!reply.ok, "a stop went through: {reply:?}");
        let why = reply.error.unwrap_or_default();
        assert!(
            why.contains("only the session that started one may stop it"),
            "it did not say why: {why}"
        );
        tidy("wall");
    }

    #[tokio::test]
    async fn claiming_to_be_the_parent_is_not_enough_to_stop_a_child() {
        // The inner gate, and the whole reason there is a secret. Every session in a project
        // runs as one user, so any process here can connect calling itself the parent — and the
        // directory, which is what decides relations, will agree with it. What it cannot do is
        // produce the secret that session was started with.
        let them = named("secret", "iota-mu");
        let me = named("secret", "kappa-rho");
        let mut about = About {
            me: them.clone(),
            parent: Some(me.id.clone()),
            token: Some("the-real-one".to_owned()),
            busy: false,
            working_for: 0,
            inbox: Vec::new(),
        };
        // The note a child leaves beside its socket, so the far end reads the caller as its
        // parent rather than as a stranger. Written by hand here; a session writes its own.
        std::fs::create_dir_all(crate::directory::home(&project("secret")))
            .expect("a project directory");
        std::fs::write(crate::directory::kin_at(&them), &me.id).expect("the note");

        let (about_tx, about_rx) = tokio::sync::watch::channel(about.clone());
        about.busy = false;
        let (arrived_tx, _arrived) = mpsc::channel(8);
        let (stopped_tx, mut stopped) = mpsc::channel(1);
        let at = crate::directory::listening_at(&them);
        tokio::spawn(async move {
            let _ = serve(
                &at,
                Serving {
                    about: about_rx,
                    arrived: arrived_tx,
                    stopped: stopped_tx,
                },
            )
            .await;
        });
        let at = crate::directory::listening_at(&them);
        for _ in 0..100 {
            if Held::at(&at, &me).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let _keep = about_tx;

        let (they, i) = (them.clone(), me.clone());
        let (guessed, right) = tokio::task::spawn_blocking(move || {
            let mut held = Held::to(&they, &i).expect("it is listening");
            (
                held.call_with("stop", Vec::new(), "guessed")
                    .expect("answered"),
                held.call_with("stop", Vec::new(), "the-real-one")
                    .expect("answered"),
            )
        })
        .await
        .expect("the thread finished");

        assert!(!guessed.ok, "a guess stopped it: {guessed:?}");
        assert!(
            guessed.error.unwrap_or_default().contains("secret"),
            "and it did not say what was wrong"
        );
        assert!(
            right.ok,
            "the parent could not stop its own child: {right:?}"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(5), stopped.recv())
                .await
                .expect("the session was told")
                .is_some()
        );
        tidy("secret");
    }

    #[tokio::test]
    async fn a_sibling_tool_speaking_the_family_shape_is_understood() {
        // Hand-written frames, the way anything that is not axon would send them. This is the
        // test the encoding bug would have failed, and the only one that could have.
        let them = named("sibling", "theta-mu");
        let _bound = listening(&them).await;
        let at = crate::directory::listening_at(&them);

        let asked = tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut sock = std::os::unix::net::UnixStream::connect(&at).expect("connected");
            sock.set_read_timeout(Some(Duration::from_secs(5)))
                .expect("a timeout");
            ["verbs", "status"]
                .into_iter()
                .map(|verb| {
                    let body = format!(r#"{{"call":"{verb}"}}"#);
                    let mut frame = u32::try_from(body.len())
                        .expect("fits")
                        .to_be_bytes()
                        .to_vec();
                    frame.extend_from_slice(body.as_bytes());
                    sock.write_all(&frame).expect("wrote");
                    let mut header = [0_u8; 4];
                    sock.read_exact(&mut header).expect("read a header");
                    let mut answer = vec![0_u8; u32::from_be_bytes(header) as usize];
                    sock.read_exact(&mut answer).expect("read a body");
                    serde_json::from_slice::<serde_json::Value>(&answer).expect("it is JSON")
                })
                .collect::<Vec<_>>()
        })
        .await
        .expect("the thread finished");

        assert_eq!(asked[0]["ok"], true, "verbs answers anybody: {}", asked[0]);
        assert!(asked[0]["result"].is_array(), "and in the family's shape");
        // Everything else is about this session, and a stranger has no standing to ask.
        assert_eq!(asked[1]["ok"], false, "status must not: {}", asked[1]);
        tidy("sibling");
    }
}

/// A stranger can fetch the vocabulary and the library that speaks it, and nothing else.
#[cfg(test)]
mod handing_over {
    use super::tests::{listening, named, tidy};
    use std::time::Duration;

    /// One hand-written call, the way anything that is not axon would send it.
    fn asked(at: &std::path::Path, verb: &str) -> serde_json::Value {
        use std::io::{Read, Write};
        let mut sock = std::os::unix::net::UnixStream::connect(at).expect("connected");
        sock.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a timeout");
        let body = format!(r#"{{"call":"{verb}"}}"#);
        let mut frame = u32::try_from(body.len())
            .expect("fits")
            .to_be_bytes()
            .to_vec();
        frame.extend_from_slice(body.as_bytes());
        sock.write_all(&frame).expect("wrote");
        let mut header = [0_u8; 4];
        sock.read_exact(&mut header).expect("read a header");
        let mut answer = vec![0_u8; u32::from_be_bytes(header) as usize];
        sock.read_exact(&mut answer).expect("read a body");
        serde_json::from_slice(&answer).expect("it is JSON")
    }

    #[tokio::test]
    async fn the_client_library_comes_back_over_the_wire() {
        // `agent lua-api` prints the same source, which is enough for a host that can shell out
        // and useless to one that cannot: a sandboxed VM with no `io.popen` has no way to run
        // it. So a sibling that speaks the framing can fetch the right vocabulary using the
        // wrong one, in code, with nothing written to disk.
        let them = named("handed", "theta-mu");
        let _bound = listening(&them).await;
        let at = crate::directory::listening_at(&them);

        let reply = tokio::task::spawn_blocking(move || asked(&at, "client"))
            .await
            .expect("the thread finished");

        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["n"], 1, "one value, in a list");
        let source = reply["result"][0].as_str().expect("source");
        assert_eq!(source, crate::CLIENT, "and it is the file this crate ships");
        tidy("handed");
    }

    #[tokio::test]
    async fn verbs_and_client_are_the_two_a_stranger_may_have() {
        // Everything else is *about this session*, and somebody who will not say who they are
        // has no standing to ask. These two are about the surface: what it speaks, and the
        // library that speaks it. Neither says anything about who is answering.
        let them = named("stranger", "iota-nu");
        let _bound = listening(&them).await;
        let at = crate::directory::listening_at(&them);

        let (open, closed) = tokio::task::spawn_blocking(move || {
            (
                [
                    asked(&at, "verbs")["ok"].clone(),
                    asked(&at, "client")["ok"].clone(),
                ],
                [
                    asked(&at, "identity")["ok"].clone(),
                    asked(&at, "status")["ok"].clone(),
                    asked(&at, "inbox")["ok"].clone(),
                    asked(&at, "stop")["ok"].clone(),
                ],
            )
        })
        .await
        .expect("the thread finished");

        assert!(open.iter().all(|ok| *ok == true), "{open:?}");
        assert!(closed.iter().all(|ok| *ok == false), "{closed:?}");
        tidy("stranger");
    }

    #[tokio::test]
    async fn every_verb_it_lists_is_one_a_client_could_call() {
        // `verbs` promising something nothing answers is worse than not listing it: a client
        // written from that list fails in somebody else's program.
        let them = named("listed", "kappa-nu");
        let _bound = listening(&them).await;
        let at = crate::directory::listening_at(&them);

        let listed = tokio::task::spawn_blocking(move || asked(&at, "verbs"))
            .await
            .expect("the thread finished");
        let named_verbs: Vec<String> = listed["result"][0]
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|entry| entry["verb"].as_str().map(ToOwned::to_owned))
            .collect();

        let known: Vec<&str> = crate::wire::VERBS.iter().map(|(verb, _)| *verb).collect();
        assert_eq!(named_verbs, known);
        tidy("listed");
    }
}
