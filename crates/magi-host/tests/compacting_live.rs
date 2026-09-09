//! Compaction, decided by the balthasar that holds the session. There is one decider now, so the
//! path can only be exercised against a real one. Skipped when balthasar is not installed: what is
//! proved here — that magi asks, obeys the span it is given, and does not go round twice — needs one.

use magi_host::scribe::Scribe;
use magi_host::session::Session;
use magi_host::turn::{Backend, run};
use magi_model::scratch::Scratch;
use magi_proto::{Entry, SessionId};
use magi_testkit::Mind;
use magi_testkit::mind::{failed_line, stop_line, text_line};

/// How long balthasar has to answer before this gives up on it.
const ANSWERS_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

/// One live balthasar at a time — the same reason `injecting_live` takes a lock.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A `balthasar serve` this test started, killed when the test ends. A guard rather than a line at
/// the bottom: a test that returns early or fails an assertion would otherwise leave one running.
struct Serving(std::process::Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A balthasar of this test's own, with its own store, and a scribe onto it. `None` when balthasar
/// is not installed, which is not a failure.
async fn own_balthasar(
    name: &str,
) -> Option<(
    Scribe,
    Scratch,
    Serving,
    tokio::sync::MutexGuard<'static, ()>,
)> {
    let held = ONE_AT_A_TIME.lock().await;
    let dir = Scratch::new("mc", name);
    let instance = format!("mc-{}-{name}", std::process::id());
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

    // Short names, because a unix socket path may not exceed `SUN_LEN` and the instance is in it.
    let socket = dir
        .join("r")
        .join("balthasar")
        .join(format!("api@{instance}.sock"));

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
    let _ = Serving(child);
    None
}

/// A backend that asks `mind` and nothing else.
fn backend(mind: &Mind) -> Backend {
    Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        casper: None,
        casper_configure: String::new(),
        cwd: std::env::temp_dir(),
        model: "fake/one".to_owned(),
        mind: mind.program().display().to_string(),
        wants: magi_proto::ask::Wants::default(),
        // The room for a conversation is `size - reserve - inject`, and balthasar's defaults take
        // 60,000 of it. So: a real window, and a conversation over the 140,000 tokens it leaves.
        context_window: Some(200_000),
        system: None,
        confine: false,
        grants: Vec::new(),
        environ: std::collections::BTreeMap::new(),
    }
}

/// A session of `count` turns, streamed to balthasar as a real one would be. The streaming is what
/// matters: balthasar refuses to plan for a session it has never been told about.
async fn conversation(scribe: &mut Scribe, count: usize) -> tokio::sync::Mutex<Session> {
    let mut session = Session::recorded(SessionId::new("s"), Vec::new());
    for i in 0..count {
        session
            .commit(Entry::User {
                id: magi_proto::MessageId::new(format!("u{i}")),
                // Big on purpose: balthasar plans on size, and twelve short turns are nothing to
                // summarise.
                text: format!(
                    "message number {i}: {}",
                    "a sentence worth counting. ".repeat(4_000)
                ),
                aside: String::new(),
            })
            .expect("commit");
    }
    for (cursor, entry) in session.take_pending() {
        let _ = scribe.observe(cursor, &entry).await;
    }
    tokio::sync::Mutex::new(session)
}

/// Run the turn against this balthasar.
async fn turn(session: &tokio::sync::Mutex<Session>, backend: &Backend, scribe: Scribe) {
    let registry = magi_tools::Registry::new();
    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    let held = std::sync::Arc::new(tokio::sync::Mutex::new(Some(scribe)));
    run(session, backend, &registry, &ops, &held)
        .await
        .expect("the turn returns");
}

#[tokio::test]
async fn a_conversation_over_budget_is_compacted_before_the_turn() {
    // The proactive path: balthasar decides, and the twelve turns below are over the 140,000 tokens
    // its plan leaves in a 200,000 window. Two arms, in the order a real run produces — the
    // compaction's own request to the model comes before the turn's.
    let Some((mut scribe, _dir, _serving, _alone)) = own_balthasar("overflow").await else {
        eprintln!("skipped: balthasar is not installed, and it decides the cut");
        return;
    };
    let mind = Mind::turns(
        "compact-overflow",
        &[
            &[&text_line("Twelve turns about counting."), &stop_line()],
            &[&text_line("The journal is append-only."), &stop_line()],
        ],
    );
    let session = conversation(&mut scribe, 12).await;
    turn(&session, &backend(&mind), scribe).await;

    let held = session.lock().await;
    let entries = held.entries();
    let compactions: Vec<&Entry> = entries
        .iter()
        .filter(|e| matches!(e, Entry::Compaction { .. }))
        .collect();
    assert_eq!(
        compactions.len(),
        1,
        "the conversation was not compacted; {} entries",
        entries.len()
    );
    // And it replaced what balthasar said to replace, rather than a constant of magi's.
    let Some(Entry::Compaction {
        replaces, summary, ..
    }) = compactions.first().copied()
    else {
        panic!("a compaction entry");
    };
    assert!(
        *replaces > 0 && *replaces < entries.len(),
        "replaces {replaces}"
    );
    assert!(!summary.is_empty(), "the summary was written by the model");

    assert!(
        matches!(entries.last(), Some(Entry::Assistant { text, .. }) if !text.is_empty()),
        "and the turn carried on to an answer: {:?}",
        entries.last()
    );
    drop(held);
}

#[tokio::test]
async fn one_prompt_is_never_compacted_twice() {
    // A conversation that still will not fit after summarising is one whose kept tail alone
    // overflows. The third arm repeats, so a loop that kept compacting would run the count away.
    let Some((mut scribe, _dir, _serving, _alone)) = own_balthasar("twice").await else {
        eprintln!("skipped: balthasar is not installed, and it decides the cut");
        return;
    };
    let overflow = failed_line("prompt is too long", "overflow");
    let summary = text_line("The user is counting sentences.");
    let stop = stop_line();
    let mind = Mind::turns(
        "compact-twice",
        &[
            &[&summary, &stop],
            &[&overflow],
            &[&summary, &stop],
            &[&overflow],
        ],
    );
    let session = conversation(&mut scribe, 12).await;
    turn(&session, &backend(&mind), scribe).await;

    let held = session.lock().await;
    let compactions = held
        .entries()
        .iter()
        .filter(|e| matches!(e, Entry::Compaction { .. }))
        .count();
    assert!(
        compactions <= 2,
        "compacted {compactions} times for one prompt: once before the turn and at most once \
         after it overflowed is the whole allowance"
    );
    // The reactive compaction is allowed exactly one go. A fourth ask would be a second one.
    assert!(
        mind.asked() <= 4,
        "it kept trying to compact: {} asks",
        mind.asked()
    );
    drop(held);
}

/// A round of tool calls, big enough that balthasar wants to mask them. `shell` because balthasar's
/// shipped config has a mask handler for it; a tool with none is deliberately left alone.
async fn tooling(scribe: &mut Scribe, count: usize) -> tokio::sync::Mutex<Session> {
    let mut session = Session::recorded(SessionId::new("s"), Vec::new());
    session
        .commit(Entry::User {
            id: magi_proto::MessageId::new("u0"),
            text: "run the tests".to_owned(),
            aside: String::new(),
        })
        .expect("commit");
    for i in 0..count {
        session
            .commit(Entry::Assistant {
                id: magi_proto::MessageId::new(format!("a{i}")),
                text: String::new(),
                thinking: String::new(),
                stop_reason: None,
                error: None,
                signatures: magi_proto::Signatures::default(),
                usage: magi_proto::Usage::default(),
            })
            .expect("commit");
        session
            .commit(Entry::Tool {
                id: magi_proto::ToolCallId::new(format!("c{i}")),
                name: "shell".to_owned(),
                args: r#"{"command":"cargo test"}"#.to_owned(),
                result: Some(magi_proto::ToolResult {
                    output: format!("run {i}: {}", "a line of test output. ".repeat(4_000)),
                    is_error: false,
                    shown: None,
                }),
                thought_signature: None,
            })
            .expect("commit");
    }
    for (cursor, entry) in session.take_pending() {
        let _ = scribe.observe(cursor, &entry).await;
    }
    tokio::sync::Mutex::new(session)
}

#[tokio::test]
async fn tool_output_is_masked_before_anything_is_summarised() {
    // balthasar tries masking first, and marks a turn masked as it hands the plan over, never
    // offering it again — so magi obeying only the summary planned against a stub it never sent.
    let Some((mut scribe, _dir, _serving, _alone)) = own_balthasar("masking").await else {
        eprintln!("skipped: balthasar is not installed, and it decides what to mask");
        return;
    };
    let mind = Mind::answering("compact-masking", "done");
    let session = tooling(&mut scribe, 8).await;
    turn(&session, &backend(&mind), scribe).await;

    let held = session.lock().await;
    let entries = held.entries();
    let masks: Vec<&Entry> = entries
        .iter()
        .filter(|e| matches!(e, Entry::Masked { .. }))
        .collect();
    assert!(
        !masks.is_empty(),
        "nothing was masked in {} entries of tool output",
        entries.len()
    );

    // The stub is the tool's own words, from balthasar's `balthasar.mask["shell"]` handler.
    let Some(Entry::Masked { shown, at, .. }) = masks.first().copied() else {
        panic!("a mask record");
    };
    assert!(
        shown.contains("shell") || shown.contains("elided"),
        "the stub did not come from the tool's handler: {shown:?}"
    );
    assert!(
        matches!(entries.get(*at), Some(Entry::Tool { .. })),
        "a mask landed on something that is not a tool result"
    );

    // And what the model was actually sent carries the stub rather than the output.
    let sent = magi_host::context::of_entries(entries);
    let bodies: Vec<String> = sent
        .messages
        .iter()
        .flat_map(|message| message.content.clone())
        .filter_map(|content| match content {
            magi_model::Content::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .collect();
    assert!(
        bodies.iter().any(|body| body == shown),
        "the stub never reached the provider"
    );
    assert!(
        bodies.iter().filter(|body| body.len() > 10_000).count() < 8,
        "every result went in full: {:?}",
        bodies.iter().map(String::len).collect::<Vec<_>>()
    );
    drop(held);
}

#[tokio::test]
async fn a_masked_session_can_still_be_read_back() {
    // What makes masking safe is that the text is still in balthasar's scrollback and something can
    // fetch it back. `scroll` is that read, and `config/tools.lua` offers it to the model as
    // `history`; this checks the verb answers for a session magi streamed.
    let Some((mut scribe, _dir, _serving, _alone)) = own_balthasar("scrolling").await else {
        eprintln!("skipped: balthasar is not installed, and it holds the history");
        return;
    };
    let _session = tooling(&mut scribe, 3).await;

    let rows = scribe.raw("scroll").await.expect("scroll answers");
    let said = serde_json::to_string(&rows).expect("json");
    assert!(
        said.contains("cargo test"),
        "the history did not come back: {}",
        &said[..said.len().min(400)]
    );
}
