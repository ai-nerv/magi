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
async fn own_balthasar(name: &str) -> Option<(Scribe, Scratch, std::process::Child)> {
    let dir = Scratch::new("magi-inject", name);
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
                return Some((scribe, dir, child));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let mut child = child;
    let _ = child.kill();
    None
}

#[tokio::test]
async fn something_remembered_comes_back_without_being_asked_for() {
    let Some((mut scribe, _dir, mut balthasar)) = own_balthasar("recalled").await else {
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

    let message = magi_host::injecting::preface(&found, WINDOW)
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

    let _ = balthasar.kill();
}

#[tokio::test]
async fn a_session_with_no_balthasar_is_told_nothing_and_still_runs() {
    // The property that lets this be unconditional. A machine without a memory layer gets the
    // session magi had before there was one, rather than an error or a wait.
    assert!(magi_host::injecting::preface(&[], WINDOW).is_none());
}
