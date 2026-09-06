//! What a project remembers, reaching the model, against a balthasar that is actually running.
//!
//! Skipped when there is no socket. What this proves cannot be proved against a mock: that a
//! thing balthasar was told is a thing a later turn is shown without anybody asking for it.
//! Every other path into the memory layer needs somebody to ask first — a surface, a tool call,
//! a `doctor` — and a model that has forgotten something cannot ask about it.

use magi_host::scribe::Scribe;
use magi_model::scratch::Scratch;
use magi_proto::SessionId;

/// How long balthasar has to answer before this gives up on it.
const ANSWERS_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

/// A window big enough that the budget is not what is under test.
const WINDOW: usize = 200_000;

/// A balthasar of this test's own, and a scribe onto it.
///
/// **Not the one the developer is using.** This writes a durable memory and reads it back, and a
/// test that put a sentence into somebody's own memory layer and walked away would make their
/// sessions worse the longer it was run. Its own store in its own directory, taken away with the
/// scratch when the test ends.
///
/// `None` when balthasar is not installed, which is the ordinary case on a machine that has not
/// got one and not a failure: this file is about the seam, not about the layer.
async fn own_balthasar(name: &str, ledger: bool) -> Option<(Scribe, Scratch, Serving)> {
    let dir = Scratch::new("magi-inject", name);
    // The ledger is off by default, and rightly: it costs writes on the recall path, and a
    // memory layer that silently started recording what a person searches for because a new
    // version shipped is not one anybody should install. It is what makes `used` and `outcome`
    // answer, so the test that exercises them turns it on for its own balthasar only.
    if ledger {
        std::fs::write(
            dir.join(".balthasar.lua"),
            "balthasar.outcome = { capture = true, retention_days = 90 }\n",
        )
        .expect("write");
    }
    let instance = format!("magi-inject-{}-{name}", std::process::id());
    let child = std::process::Command::new("balthasar")
        .arg("serve")
        .arg("--instance")
        .arg(&instance)
        .arg("--scope")
        .arg("project")
        .current_dir(&*dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let socket = runtime
        .join("balthasar")
        .join(format!("api@{instance}.sock"));

    // Polled rather than slept on: binding is the first thing it does, and a fixed wait is
    // either too short on a loaded machine or wasted on an idle one.
    let id = SessionId::new(&instance);
    let deadline = std::time::Instant::now() + ANSWERS_WITHIN;
    while std::time::Instant::now() < deadline {
        if let Ok(family) = magi_ipc::family::Family::dial(&socket).await {
            let mut scribe = Scribe::over(family, Some(socket.clone()), &id);
            // Answering, not merely bound: a socket file outlives the process that made it.
            if let Ok(Ok(_)) = tokio::time::timeout(ANSWERS_WITHIN, scribe.replay()).await {
                return Some((scribe, dir, Serving(child)));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let _ = Serving(child);
    None
}

/// A `balthasar serve` this test started, killed when the test ends.
///
/// **A guard, not a line at the bottom.** The first version killed the child on the last line of
/// each test, and a test that returned early — no balthasar installed — or failed an assertion
/// left one running: a `verify` on this machine found nine of them, and the suite that started
/// them took half an hour. It is the same failure the trailing `remove_dir_all` was, in a
/// process instead of a directory.
struct Serving(std::process::Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn something_remembered_comes_back_without_being_asked_for() {
    let Some((mut scribe, _dir, _balthasar)) = own_balthasar("recalled", false).await else {
        // The ordinary case on a machine with no balthasar, and the session magi had before
        // there was one. Not a failure: this file is about the seam, not about the layer.
        eprintln!("no balthasar is answering; skipped");
        return;
    };

    // Kept the way a durable memory is kept. Deliberately not `observe`: observing writes a
    // run's *scratch*, which is that run's own until something on the ladder carries it across,
    // and a recall does not return it. That is balthasar's design and it is the right one — what
    // gets put in front of a model unasked should be established, not the last thing said.
    let phrase = format!(
        "the deploy command here is `make ship-{}`",
        std::process::id()
    );
    scribe.keep(&phrase).await.expect("balthasar keeps it");

    // Asked for by nobody: this is the query a later turn would build from the prompt in front
    // of it, and the whole point is that the model never had to think of it.
    let found = tokio::time::timeout(ANSWERS_WITHIN, scribe.nearest("deploy command", 12))
        .await
        .expect("balthasar answers")
        .expect("a recall");

    let message = magi_host::injecting::preface(&found.memories, WINDOW)
        .expect("what was kept a moment ago is what a turn is shown");

    let said: String = message
        .content
        .iter()
        .filter_map(|c| match c {
            magi_model::Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        said.contains("remembers"),
        "the block says what it is: {said}"
    );
    assert!(
        said.contains(&phrase),
        "the thing that was kept is the thing the turn is shown: {said}"
    );
}

#[tokio::test]
async fn a_session_with_no_balthasar_is_told_nothing_and_still_runs() {
    // The property that lets this be unconditional. A machine without a memory layer gets the
    // session magi had before there was one, rather than an error or a wait.
    assert!(magi_host::injecting::preface(&[], WINDOW).is_none());
}

#[tokio::test]
async fn what_the_turn_did_next_goes_back_to_the_memory_layer() {
    // **The loop that decides whether a memory was any good.** Everything else is one direction:
    // the transcript goes over, memories come back. This is the only signal balthasar has for
    // whether anything it offered was worth offering — without it, a memory layer ranks by
    // recency and similarity forever.
    //
    // It is also the axis MemoryArena separates from LoCoMo, and the one neither pi nor deepseek
    // has anything for.
    let Some((mut scribe, _dir, _balthasar)) = own_balthasar("outcome", true).await else {
        eprintln!("no balthasar is installed; skipped");
        return;
    };

    scribe
        .keep("the deploy command here is `oslo make install`")
        .await
        .expect("balthasar keeps it");

    let found = tokio::time::timeout(ANSWERS_WITHIN, scribe.nearest("deploy", 12))
        .await
        .expect("balthasar answers")
        .expect("a recall");

    // With the ledger on, a recall is an *injection*: memories handed to something about to put
    // them in a model's context. The id is what makes an outcome attributable to them.
    let injection = found
        .injection
        .expect("a balthasar keeping a ledger says which injection these came from");
    assert!(
        !found.memories.is_empty(),
        "and hands the memories over too"
    );

    // What the turn then did. balthasar decides for itself whether the action followed from any
    // of the memories; magi reports the event and nothing more.
    let outcome = scribe
        .acted(&injection, "shell", "oslo make install", true)
        .await
        .expect("balthasar takes the report");

    // **Recorded, not merely accepted.** balthasar answers the row it wrote, and the two replies
    // are otherwise identical — a `used` against an injection it never served, or a ledger that
    // is off, comes back just as `ok`. The id is the difference between a closed loop and a call
    // that went nowhere.
    assert!(
        outcome.is_some_and(|id| id.contains("outcome")),
        "the outcome was written down, not just acknowledged"
    );
}

#[tokio::test]
async fn balthasar_serves_the_library_that_speaks_it() {
    // **A consumer keeping its own copy is a consumer whose copy goes stale**, and this one did:
    // magi's copy of the client library predated a fix to the connect path, so every session on
    // that machine silently had no memory tools and nothing anywhere said why. Connect with the
    // copy you have, take the one the server serves.
    let Some((mut scribe, _dir, _balthasar)) = own_balthasar("library", false).await else {
        eprintln!("no balthasar is installed; skipped");
        return;
    };

    let served = tokio::time::timeout(ANSWERS_WITHIN, scribe.library())
        .await
        .expect("balthasar answers")
        .expect("it serves its own library");

    assert!(
        served.contains("client library"),
        "it is the file balthasar ships: {served:.120}"
    );
    // The property that matters: what it serves is what it is actually running, so a consumer
    // taking this cannot be holding a copy older than the server.
    assert!(
        served.contains("FAMILY"),
        "and it is current — the wire version is in it"
    );
}

#[tokio::test]
async fn balthasar_says_where_it_thinks_the_session_left_off() {
    // **A cross-check, not a source.** magi's journal is the copy of record, so resuming from
    // balthasar would mean rebuilding the transcript from a projection of itself. What this is
    // for is the disagreement: a balthasar that holds none of a session's turns is one whose
    // `plan`, `replay` and `scroll` are all answering about a different conversation, and until
    // now there was no way to notice.
    let Some((mut scribe, _dir, _balthasar)) = own_balthasar("resume", false).await else {
        eprintln!("no balthasar is installed; skipped");
        return;
    };

    let held = tokio::time::timeout(ANSWERS_WITHIN, scribe.resumes())
        .await
        .expect("balthasar answers")
        .expect("resume");
    assert_eq!(
        held, 0,
        "a session it has never been told about holds nothing"
    );

    scribe
        .observe(
            magi_proto::Cursor(1),
            &magi_proto::Entry::User {
                id: magi_proto::MessageId::new("u1"),
                text: "the first thing anybody said".to_owned(),
                aside: String::new(),
            },
        )
        .await
        .expect("balthasar takes the turn");

    let after = tokio::time::timeout(ANSWERS_WITHIN, scribe.resumes())
        .await
        .expect("balthasar answers")
        .expect("resume");
    assert!(after > held, "a turn it was told about is a turn it holds");
}

#[tokio::test]
async fn a_balthasar_that_never_answers_does_not_hold_up_a_session() {
    // **The bug this exists to keep out.** Two cross-checks are asked while a session is
    // starting — where balthasar thinks the session left off, and the library it serves — and
    // both were written without a clock. A balthasar that accepts the connection and then thinks
    // about it held up every session in the suite: sixty seconds a test, and nothing said why.
    //
    // Neither answer is needed for a session to run. A socket that accepts and never replies is
    // the ordinary shape of a wedged process, and it is bound here on purpose.
    let dir = Scratch::new("magi-inject", "wedged");
    let path = dir.join("api@wedged.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");

    let mut scribe = magi_host::scribe::Scribe::over(
        magi_ipc::family::Family::dial(&path)
            .await
            .expect("it accepts, as a wedged one does"),
        Some(path.clone()),
        &SessionId::new("wedged"),
    );

    // Each call must give up rather than wait. The session's own budget is half a second; this
    // allows several times that before calling it a hang, so a slow machine is not a failure.
    let started = std::time::Instant::now();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), scribe.resumes()).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), scribe.library()).await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "a wedged balthasar held the session for {:?}",
        started.elapsed()
    );
}
