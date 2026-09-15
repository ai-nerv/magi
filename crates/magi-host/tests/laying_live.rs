//! Laying a request out, against a balthasar of each test's own: magi asks, sends what it is told,
//! and says so. Skipped when balthasar is not installed, or is one from before layouts.

use magi_host::scribe::Scribe;
use magi_host::session::Session;
use magi_host::turn::{Backend, run};
use magi_model::scratch::Scratch;
use magi_proto::{Entry, HarnessEvent, MessageId, SessionId, StopReason, ToolCallId, ToolResult};
use magi_testkit::Mind;
use magi_testkit::memory::Serving;

/// One live balthasar at a time, as the other live suites take.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A balthasar of this test's own, a scribe onto it, and the session it records.
struct Live {
    scribe: magi_host::scribe::Held,
    id: SessionId,
    /// Taken to kill it, for the test of a balthasar that dies mid-session.
    serving: Option<Serving>,
    _dir: Scratch,
    _alone: tokio::sync::MutexGuard<'static, ()>,
}

async fn live(name: &str) -> Option<Live> {
    let alone = ONE_AT_A_TIME.lock().await;
    let dir = Scratch::new("ll", name);
    let instance = format!("l{}-{name}", std::process::id());
    let Some(serving) = Serving::start(&dir, &instance).await else {
        eprintln!("skipping: no balthasar is installed");
        return None;
    };
    let mut family = magi_ipc::family::Family::dial(serving.socket())
        .await
        .expect("dial the balthasar that just answered");
    let verbs = family.call("verbs", Vec::new()).await.unwrap_or_default();
    if !serde_json::Value::Array(verbs)
        .to_string()
        .contains("\"layout\"")
    {
        eprintln!("skipping: this balthasar does not lay out");
        return None;
    }
    let id = SessionId::new(&instance);
    let scribe = Scribe::over(family, Some(serving.socket().to_owned()), &id);
    Some(Live {
        scribe: std::sync::Arc::new(tokio::sync::Mutex::new(Some(scribe))),
        id,
        serving: Some(serving),
        _dir: dir,
        _alone: alone,
    })
}

fn backend(mind: &Mind, window: u64) -> Backend {
    Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        tooling: Default::default(),
        cwd: std::env::temp_dir(),
        model: "fake/one".to_owned(),
        mind: mind.program().display().to_string(),
        wants: magi_proto::ask::Wants {
            max_tokens: Some(1_000),
            ..Default::default()
        },
        context_window: Some(window),
        system: None,
        confine: false,
        isolate: false,
        grants: Vec::new(),
        environ: std::collections::BTreeMap::new(),
        helpers: Default::default(),
    }
}

fn user(id: &str, text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(id),
        text: text.into(),
        aside: String::new(),
    }
}

fn said(id: &str, text: &str, stop: StopReason) -> Entry {
    Entry::Assistant {
        id: MessageId::new(id),
        text: text.into(),
        thinking: String::new(),
        stop_reason: Some(stop),
        error: None,
        signatures: Default::default(),
        usage: Default::default(),
    }
}

/// Run one prompt and hand back the layouts it reported.
async fn prompt(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    live: &Live,
) -> Vec<(String, magi_proto::Laid)> {
    let mut events = session.lock().await.subscribe();
    let registry = magi_tools::Registry::new();
    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    run(session, backend, &registry, &ops, &live.scribe)
        .await
        .expect("the turn returns");
    let mut laid = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let HarnessEvent::ContextLaid { id, counts, .. } = event {
            laid.push((id, counts));
        }
    }
    laid
}

#[tokio::test]
async fn every_request_is_one_balthasar_laid_out() {
    let Some(live) = live("laid").await else {
        return;
    };
    let session = tokio::sync::Mutex::new(Session::recorded(live.id.clone(), Vec::new()));
    session
        .lock()
        .await
        .commit(user("u1", "what is magi"))
        .expect("commit");
    let mind = Mind::answering("ll-laid", "a harness");

    let laid = prompt(&session, &backend(&mind, 200_000), &live).await;
    let (id, counts) = laid.first().expect("the request was reported as laid out");
    assert!(!id.is_empty(), "balthasar laid it out, not magi's fallback");
    assert_eq!(counts.items, 1, "{counts:?}");
    assert!(mind.heard().contains("what is magi"), "{}", mind.heard());
}

#[tokio::test]
async fn a_balthasar_that_dies_mid_session_only_degrades_the_layout() {
    let Some(mut live) = live("dies").await else {
        return;
    };
    let session = tokio::sync::Mutex::new(Session::recorded(live.id.clone(), Vec::new()));
    session
        .lock()
        .await
        .commit(user("u1", "what is magi"))
        .expect("commit");
    let mind = Mind::answering("ll-dies", "still here");
    let backend = backend(&mind, 200_000);
    let first = prompt(&session, &backend, &live).await;
    assert!(!first[0].0.is_empty(), "balthasar laid out the first");

    drop(live.serving.take());
    session
        .lock()
        .await
        .commit(user("u3", "are you still there?"))
        .expect("commit");
    let second = prompt(&session, &backend, &live).await;
    let (id, counts) = second.first().expect("a layout was still reported");
    assert!(id.is_empty(), "made by magi: nobody was left to ask");
    assert_eq!(counts.items, 3, "the last layout and everything since");
    let last = mind.asks().pop().unwrap_or_default();
    assert!(last.contains("are you still there?"), "the prompt went");
    assert!(last.contains("what is magi"), "and what came before it");
}

#[tokio::test]
async fn a_screen_is_told_the_notes_and_their_change_log() {
    let Some(live) = live("notes").await else {
        return;
    };
    let said = magi_host::knowing::notes(&live.scribe, "notes", serde_json::json!({})).await;
    let verbs: Vec<&str> = said
        .iter()
        .filter_map(|event| match event {
            HarnessEvent::MemoryAnswered { verb, .. } => Some(verb.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(verbs, ["notes", "changes"], "{said:?}");
}

#[tokio::test]
async fn a_long_conversation_is_summarised_by_a_helper_and_the_summary_is_sent() {
    let Some(live) = live("summary").await else {
        return;
    };
    // Twelve exchanges of three thousand tokens in a forty-thousand window: past where balthasar
    // asks for a summary, with nothing it could stub instead.
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(Session::recorded(
        live.id.clone(),
        Vec::new(),
    )));
    {
        let mut held = session.lock().await;
        for n in 0..12 {
            let long = format!("PART{n} ").repeat(900);
            held.commit(user(&format!("u{n}"), &long)).expect("commit");
            held.commit(said(&format!("a{n}"), &long, StopReason::EndTurn))
                .expect("commit");
        }
        held.commit(user("u12", "and the next thing"))
            .expect("commit");
    }
    let mind = Mind::answering("ll-summary", "SUMMARY-OF-EARLIER");
    let backend = backend(&mind, 40_000);
    let mut events = session.lock().await.subscribe();
    prompt(&session, &backend, &live).await;

    // The summary is a background job, run once the turn is over, on the session's own model: no
    // helper is configured and the job falls back to it.
    magi_host::helping::between(
        std::sync::Arc::clone(&session),
        backend.clone(),
        std::sync::Arc::clone(&live.scribe),
    );
    let helped = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            if let Ok(HarnessEvent::HelperSpent { role, .. }) = events.recv().await {
                return role;
            }
        }
    })
    .await
    .expect("a helper job ran after the turn");
    assert_eq!(helped, "memory");
    // `job_done` follows the spend on the same task.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    session
        .lock()
        .await
        .commit(user("u13", "what came before?"))
        .expect("commit");
    let laid = prompt(&session, &backend, &live).await;
    let (_, counts) = laid.first().expect("a layout");
    assert_eq!(counts.summary, 1, "the summary is a slot: {counts:?}");
    let last = mind.asks().pop().unwrap_or_default();
    assert!(
        last.contains("SUMMARY-OF-EARLIER"),
        "the summary was not sent"
    );
    assert!(
        !last.contains(&"PART0 ".repeat(50)),
        "what it covers went too"
    );
}

#[tokio::test]
async fn a_request_refused_as_too_long_is_laid_out_again_and_retried() {
    let Some(live) = live("over").await else {
        return;
    };
    let session = tokio::sync::Mutex::new(Session::recorded(live.id.clone(), Vec::new()));
    session
        .lock()
        .await
        .commit(user("u1", "what is magi"))
        .expect("commit");
    let refused = magi_testkit::mind::failed_line(
        "prompt is too long: 250000 tokens > 200000 maximum",
        "overflow",
    );
    let answered = [
        magi_testkit::mind::text_line("a harness"),
        magi_testkit::mind::stop_line(),
    ];
    let mind = Mind::turns(
        "ll-over",
        &[
            &[refused.as_str()],
            &[answered[0].as_str(), answered[1].as_str()],
        ],
    );

    let laid = prompt(&session, &backend(&mind, 200_000), &live).await;
    assert_eq!(mind.asked(), 2, "refused once, then answered");
    assert_eq!(laid.len(), 2, "one layout, then a tighter one: {laid:?}");
    assert!(!laid[1].0.is_empty(), "balthasar answered the overflow");
    assert_ne!(laid[0].0, laid[1].0, "a new layout, not the same one");
}

#[tokio::test]
async fn an_old_result_too_big_for_the_window_goes_as_its_stub() {
    let Some(live) = live("big").await else {
        return;
    };
    // Four reads of seven thousand tokens each: past where balthasar prunes in a forty-thousand
    // window, with the oldest outside the last three results it keeps word for word.
    let output = |n: usize| format!("RESULT{n} ").repeat(3_500);
    let session = tokio::sync::Mutex::new(Session::recorded(live.id.clone(), Vec::new()));
    {
        let mut held = session.lock().await;
        held.commit(user("u0", "read the four parts"))
            .expect("commit");
        for n in 1..=4 {
            let id = format!("c{n}");
            held.commit(said(&format!("a{n}"), "", StopReason::ToolUse))
                .expect("commit");
            held.commit(Entry::Tool {
                id: ToolCallId::new(&id),
                name: "read".into(),
                args: format!("{{\"path\":\"part{n}.txt\"}}"),
                result: Some(ToolResult {
                    output: output(n),
                    is_error: false,
                    shown: None,
                }),
                thought_signature: None,
            })
            .expect("commit");
            held.hint(
                &id,
                magi_proto::tooling::Hints {
                    brief: Some(format!("read part{n}.txt (3000 lines)")),
                    back: Some(format!("read part{n}.txt")),
                    keep: false,
                },
            );
        }
        held.commit(said("a9", "all four are read", StopReason::EndTurn))
            .expect("commit");
        held.commit(user("u10", "now summarise them"))
            .expect("commit");
    }
    let mind = Mind::answering("ll-big", "short");

    let laid = prompt(&session, &backend(&mind, 40_000), &live).await;
    let (_, counts) = laid.first().expect("a layout was reported");
    let heard = mind.heard();
    assert!(heard.contains("now summarise them"), "{heard}");
    assert!(counts.stubs > 0, "nothing was stubbed: {counts:?}");
    assert!(
        heard.contains("read part1.txt (3000 lines)"),
        "the stub is not casper's words"
    );
    assert!(
        !heard.contains(&output(1)[..800]),
        "the oldest result went whole: {counts:?}"
    );
    assert!(
        heard.contains(&output(4)[..800]),
        "the newest result is kept word for word"
    );
}
