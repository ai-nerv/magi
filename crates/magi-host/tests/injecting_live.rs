//! What a project remembers, reaching the model, against a balthasar that is actually running.
//! Skipped when there is no socket. What this proves cannot be proved against a mock: that a thing
//! balthasar was told is shown to a later turn without anybody asking for it.

use magi_host::scribe::Scribe;
use magi_model::scratch::Scratch;
use magi_proto::SessionId;

/// How long balthasar has to answer before this gives up on it: the scribe's own durable clock, so
/// this never cuts short a call the scribe would still be waiting on.
const ANSWERS_WITHIN: std::time::Duration = magi_ipc::family::DURABLE;

/// A window big enough that the budget is not what is under test.
const WINDOW: usize = 200_000;

/// One live balthasar at a time. Each test here starts a server and `cargo test` runs them at once;
/// seven of them racing the scribe's deadline is a suite that passes alone and fails in a workspace run.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A balthasar of this test's own, and a scribe onto it — not the one the developer is using, since
/// this writes a durable memory. `None` when balthasar is not installed, which is not a failure:
/// this file is about the seam, not about the layer.
async fn own_balthasar(
    name: &str,
    ledger: bool,
) -> Option<(
    Scribe,
    Scratch,
    Serving,
    tokio::sync::MutexGuard<'static, ()>,
)> {
    // Taken before anything is started, and handed back so it is held for the test's own body.
    let held = ONE_AT_A_TIME.lock().await;
    let dir = Scratch::new("mi", name);
    // The ledger is off by default and costs writes on the recall path; `used` and `outcome` need it.
    if ledger {
        std::fs::write(
            dir.join(".balthasar.lua"),
            "balthasar.outcome = { capture = true, retention_days = 90 }\n",
        )
        .expect("write");
    }
    let instance = format!("mi-{}-{name}", std::process::id());
    let child = std::process::Command::new("balthasar")
        .arg("serve")
        .arg("--instance")
        .arg(&instance)
        .arg("--scope")
        .arg("project")
        .current_dir(&*dir)
        // The runtime directory is the scratch's too. balthasar binds a socket under it and the
        // `SIGKILL` in `Serving` gives it no chance to unlink one, so a real directory fills up.
        .env("XDG_RUNTIME_DIR", dir.join("r"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    // Short names, because the instance appears inside a socket path bounded by `SUN_LEN`.
    let socket = dir
        .join("r")
        .join("balthasar")
        .join(format!("api@{instance}.sock"));

    // Polled rather than slept on: a fixed wait is too short when loaded and wasted when idle.
    let id = SessionId::new(&instance);
    let deadline = std::time::Instant::now() + ANSWERS_WITHIN;
    while std::time::Instant::now() < deadline {
        if let Ok(family) = magi_ipc::family::Family::dial(&socket).await {
            let mut scribe = Scribe::over(family, Some(socket.clone()), &id);
            // Answering, not merely bound: a socket file outlives the process that made it.
            if let Ok(Ok(_)) = tokio::time::timeout(ANSWERS_WITHIN, scribe.replay()).await {
                return Some((scribe, dir, Serving(child), held));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    // Installed and started, and never answered. Not a skip: reporting "not installed" here let a
    // slow machine pass this file without it having tested anything.
    drop(Serving(child));
    panic!("balthasar started and did not answer within {ANSWERS_WITHIN:?}");
}

/// A `balthasar serve` this test started, killed when the test ends. A guard, not a line at the
/// bottom: a test that returns early or fails an assertion would otherwise leave one running.
struct Serving(std::process::Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn something_remembered_comes_back_without_being_asked_for() {
    let Some((mut scribe, _dir, _balthasar, _held)) = own_balthasar("recalled", false).await else {
        // The ordinary case on a machine with no balthasar, and not a failure.
        eprintln!("no balthasar is answering; skipped");
        return;
    };

    // Kept the way a durable memory is kept. Deliberately not `observe`: observing writes a run's
    // scratch, which is that run's own and which a recall does not return.
    let phrase = format!(
        "the deploy command here is `make ship-{}`",
        std::process::id()
    );
    scribe.keep(&phrase).await.expect("balthasar keeps it");

    // Asked for by nobody: the query a later turn would build from the prompt in front of it.
    let found = tokio::time::timeout(ANSWERS_WITHIN, scribe.nearest("deploy command", 12))
        .await
        .expect("balthasar answers")
        .expect("a recall");

    let offers = vec![magi_host::injecting::offered("balthasar", &found.memories)];
    let message = magi_host::supplying::pack(&offers, WINDOW)
        .message
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
        said.contains("not part of the conversation"),
        "the block says what it is: {said}"
    );
    // Cited either way: asserted blocks under a supplier heading, hedged ones with the supplier named.
    assert!(
        said.contains("balthasar"),
        "and which supplier said it: {said}"
    );
    assert!(
        said.contains(&phrase),
        "the thing that was kept is the thing the turn is shown: {said}"
    );
}

#[tokio::test]
async fn a_session_with_no_balthasar_is_told_nothing_and_still_runs() {
    assert!(magi_host::supplying::pack(&[], WINDOW).message.is_none());
}

#[tokio::test]
async fn what_the_turn_did_next_goes_back_to_the_memory_layer() {
    // The only signal balthasar has for whether anything it offered was worth offering. Without it
    // a memory layer ranks by recency and similarity forever.
    let Some((mut scribe, _dir, _balthasar, _held)) = own_balthasar("outcome", true).await else {
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

    // With the ledger on a recall is an injection, and the id is what makes an outcome attributable.
    let injection = found
        .injection
        .expect("a balthasar keeping a ledger says which injection these came from");
    assert!(
        !found.memories.is_empty(),
        "and hands the memories over too"
    );

    // balthasar decides whether the action followed from any of the memories; magi only reports it.
    let outcome = scribe
        .acted(&injection, "shell", "oslo make install", true)
        .await
        .expect("balthasar takes the report");

    // Recorded, not merely accepted: a `used` against an injection it never served comes back `ok`
    // too, so the id is the difference between a closed loop and a call that went nowhere.
    assert!(
        outcome.is_some_and(|id| id.contains("outcome")),
        "the outcome was written down, not just acknowledged"
    );
}

#[tokio::test]
async fn balthasar_serves_the_library_that_speaks_it() {
    // A consumer keeping its own copy is a consumer whose copy goes stale: magi's copy of the
    // client library once predated a fix, so sessions silently had no memory tools. Take the served one.
    let Some((mut scribe, _dir, _balthasar, _held)) = own_balthasar("library", false).await else {
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
    // What it serves is what it is running, so a consumer cannot hold a copy older than the server.
    assert!(
        served.contains("FAMILY"),
        "and it is current — the wire version is in it"
    );
}

#[tokio::test]
async fn balthasar_says_where_it_thinks_the_session_left_off() {
    // A cross-check, not a source: magi's journal is the copy of record. What this is for is the
    // disagreement — a balthasar holding none of a session's turns is answering about another one.
    let Some((mut scribe, _dir, _balthasar, _held)) = own_balthasar("resume", false).await else {
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
    // Two cross-checks are asked while a session starts, and both were written without a clock. A
    // socket that accepts and never replies is the ordinary shape of a wedged process.
    let dir = Scratch::new("mi", "wedged");
    let path = dir.join("api@wedged.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");

    let mut scribe = magi_host::scribe::Scribe::over(
        magi_ipc::family::Family::dial(&path)
            .await
            .expect("it accepts, as a wedged one does"),
        Some(path.clone()),
        &SessionId::new("wedged"),
    );

    // Each call must give up rather than wait; this allows several times the session's own budget.
    let started = std::time::Instant::now();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), scribe.resumes()).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), scribe.library()).await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "a wedged balthasar held the session for {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_permission_reaches_the_memory_layer_as_a_trace_row() {
    // A permission is not a transcript entry: it happens around the transcript rather than in it,
    // and before this it reached only the session's own VM and nothing that outlives the process.
    let Some((mut scribe, _dir, _serving, _held)) = own_balthasar("trace", false).await else {
        eprintln!("skipping: no balthasar");
        return;
    };

    scribe
        .noticed(
            magi_proto::Cursor(1),
            "permission",
            "run `git status` was allowed",
        )
        .await
        .expect("balthasar takes a trace row");

    // Not `Scribe::replay`, which deserialises into `Entry`; a trace row is deliberately not one.
    let rows = scribe
        .raw("replay")
        .await
        .expect("balthasar replays what it was told");
    let said = format!("{rows:?}");
    assert!(said.contains("git status"), "the row came back: {said}");
    assert!(said.contains("permission"), "as a trace row: {said}");
}
