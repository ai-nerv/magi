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
    _serving: Serving,
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
        _serving: serving,
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
async fn a_result_too_big_for_the_window_is_not_sent_whole() {
    let Some(live) = live("big").await else {
        return;
    };
    let whole = "BIGLINE ".repeat(8_000);
    let session = tokio::sync::Mutex::new(Session::recorded(live.id.clone(), Vec::new()));
    {
        let mut held = session.lock().await;
        held.commit(user("u1", "read big.txt")).expect("commit");
        held.commit(said("a2", "", StopReason::ToolUse))
            .expect("commit");
        held.commit(Entry::Tool {
            id: ToolCallId::new("c1"),
            name: "read".into(),
            args: "{\"path\":\"big.txt\"}".into(),
            result: Some(ToolResult {
                output: whole.clone(),
                is_error: false,
                shown: None,
            }),
            thought_signature: None,
        })
        .expect("commit");
        held.hint(
            "c1",
            magi_proto::tooling::Hints {
                brief: Some("read big.txt (1000 lines)".into()),
                back: Some("read big.txt".into()),
                keep: false,
            },
        );
        held.commit(said("a4", "it is big", StopReason::EndTurn))
            .expect("commit");
        held.commit(user("u5", "now summarise it")).expect("commit");
    }
    let mind = Mind::answering("ll-big", "short");

    let laid = prompt(&session, &backend(&mind, 12_000), &live).await;
    let (_, counts) = laid.first().expect("a layout was reported");
    let heard = mind.heard();
    assert!(heard.contains("now summarise it"), "{heard}");
    assert!(
        !heard.contains(&whole[..800]),
        "sixteen thousand tokens went whole into a twelve-thousand window: {counts:?}"
    );
    assert!(counts.stubs + counts.dropped > 0, "{counts:?}");
    if counts.stubs > 0 {
        assert!(heard.contains("read big.txt (1000 lines)"), "{heard}");
    }
}
