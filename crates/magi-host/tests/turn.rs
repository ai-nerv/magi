//! A whole turn, against a melchior that answers.
//!
//! No account and no network: a script on disk plays the part, so the path a real turn takes —
//! ask built, melchior spawned, answer read a line at a time, deltas folded, entry amended,
//! handed to the store — is exercised end to end.
//!
//! What is *not* here any more is retry policy and HTTP status classification. melchior owns
//! both, and they have their own tests over there. What is left is the half magi still decides:
//! what it does with an answer, a refusal, an overflow and an interrupt.

use magi_model::scratch::Scratch;

use magi_host::session::Session;
use magi_host::turn::{Backend, run};
use magi_proto::{Entry, SessionId, StopReason};
use magi_testkit::Mind;
use magi_testkit::mind::{failed_line, retrying_line, stop_line, text_line};

/// A backend that asks `mind` and nothing else.
fn backend(mind: &Mind) -> Backend {
    Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        tooling: Default::default(),
        cwd: std::env::temp_dir(),
        model: "fake/one".to_owned(),
        mind: mind.program().display().to_string(),
        wants: magi_proto::ask::Wants::default(),
        context_window: Some(200_000),
        system: None,
        confine: false,
        grants: Vec::new(),
        environ: std::collections::BTreeMap::new(),
    }
}

fn session(name: &str) -> (tokio::sync::Mutex<Session>, Scratch) {
    let dir = Scratch::new("magi-turn", name);

    let session = Session::recorded(SessionId::new("s"), Vec::new());
    (tokio::sync::Mutex::new(session), dir)
}

/// Run the turn with nothing to call, which is every test here but the tool ones.
async fn turn(session: &tokio::sync::Mutex<Session>, backend: &Backend) {
    let registry = magi_tools::Registry::new();
    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    // No memory layer: these tests are about the turn loop, and a balthasar answering here
    // would make what the model is shown depend on what this machine happens to remember.
    let scribe = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    run(session, backend, &registry, &ops, &scribe)
        .await
        .expect("the turn returns");
}

#[tokio::test]
async fn a_turn_streams_into_the_transcript_and_is_queued_for_the_store() {
    let (session, _dir) = session("ok");
    let mind = Mind::saying(
        "turn-ok",
        &[
            &serde_json::json!({ "event": "thinking", "text": "weighing it" }).to_string(),
            &serde_json::json!({ "event": "signature", "signature": "sig-abc" }).to_string(),
            &text_line("The journal "),
            &text_line("is append-only."),
            &stop_line(),
        ],
    );
    turn(&session, &backend(&mind)).await;

    let mut held = session.lock().await;
    let entries = held.entries();
    assert_eq!(entries.len(), 1, "one assistant entry, amended in place");
    let Entry::Assistant {
        text,
        thinking,
        stop_reason,
        error,
        signatures,
        ..
    } = &entries[0]
    else {
        panic!("expected an assistant entry, got {:?}", entries[0]);
    };
    assert_eq!(text, "The journal is append-only.");
    assert_eq!(thinking, "weighing it");
    assert_eq!(*stop_reason, Some(StopReason::EndTurn));
    assert!(error.is_none());
    // Byte for byte. A signature the model will refuse on the next request if it is altered is
    // the whole reason the wire between magi and melchior carries opaque strings intact.
    assert_eq!(signatures.thinking.as_deref(), Some("sig-abc"));

    // **And it is queued for the store, not just held on screen.** This read the sentence back
    // out of a JSONL file on disk; there is no file, because balthasar is the store and magi
    // keeping a second copy was a copy that goes stale. What is queued is what the scribe hands
    // over, so this is the same claim at the seam it now crosses.
    let queued = held.take_pending();
    assert!(
        queued.iter().any(|(_, entry)| matches!(
            entry,
            Entry::Assistant { text, .. } if text.contains("append-only")
        )),
        "the turn was never handed to the store: {queued:?}"
    );

    drop(held);
}

#[tokio::test]
async fn what_the_model_was_asked_is_the_conversation_so_far() {
    // The ask is built here and read there, and a context that never left the struct looks
    // identical from the outside: the turn runs, the answer arrives, nothing complains.
    let (session, _dir) = session("asked");
    session
        .lock()
        .await
        .commit(Entry::User {
            id: magi_proto::MessageId::new("u1"),
            text: "what holds the transcript?".into(),
            aside: String::new(),
        })
        .expect("commit");
    let mind = Mind::answering("turn-asked", "balthasar does");
    let mut backend = backend(&mind);
    backend.system = Some("You are magi.".to_owned());
    turn(&session, &backend).await;

    let heard = mind.heard();
    assert!(heard.contains("what holds the transcript?"), "{heard}");
    assert!(heard.contains("You are magi."), "{heard}");
    assert!(heard.contains("fake/one"), "the model is named: {heard}");
}

#[tokio::test]
async fn a_refusal_becomes_a_well_formed_entry() {
    // Errors are values: the transcript stays uniform and the UI needs no error branch.
    let (session, _dir) = session("err");
    let mind = Mind::saying("turn-err", &[&failed_line("529 Overloaded", "overload")]);
    turn(&session, &backend(&mind)).await;

    let held = session.lock().await;
    let Entry::Assistant {
        stop_reason, error, ..
    } = &held.entries()[0]
    else {
        panic!("expected an assistant entry");
    };
    assert_eq!(*stop_reason, Some(StopReason::Error));
    assert!(
        error.as_deref().unwrap_or_default().contains("529"),
        "{error:?}"
    );

    drop(held);
}

#[tokio::test]
async fn a_mind_that_stops_mid_sentence_is_named_rather_than_waited_on() {
    // Silence is the one answer nobody can read. A melchior that exits without a terminal is a
    // broken sibling, and a turn that reported success would leave an empty message on screen
    // with nothing anywhere to say why.
    let (session, _dir) = session("silence");
    let mind = Mind::saying("turn-silence", &[&text_line("half a th")]);
    turn(&session, &backend(&mind)).await;

    let held = session.lock().await;
    let Entry::Assistant {
        stop_reason, error, ..
    } = &held.entries()[0]
    else {
        panic!("expected an assistant entry");
    };
    assert_eq!(*stop_reason, Some(StopReason::Error));
    assert!(
        error
            .as_deref()
            .unwrap_or_default()
            .contains("without finishing"),
        "{error:?}"
    );
    drop(held);
}

#[tokio::test]
async fn the_turn_ends_idle_whatever_happened() {
    // A status that never changes is indistinguishable from a hang.
    let (session, _dir) = session("idle");
    let mind = Mind::saying("turn-idle", &[&failed_line("nothing works", "unknown")]);
    turn(&session, &backend(&mind)).await;

    assert_eq!(
        *session.lock().await.status(),
        magi_proto::AgentStatus::Idle
    );
}

#[tokio::test]
async fn an_interrupt_stops_a_turn_the_model_has_not_finished() {
    let (session, _dir) = session("cancel");
    let mind = Mind::silent("turn-cancel");

    let cancel = session.lock().await.cancel();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        cancel.request();
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        turn(&session, &backend(&mind)),
    )
    .await
    .expect("the turn gives up rather than waiting the mind out");

    let held = session.lock().await;
    let Entry::Assistant {
        stop_reason, error, ..
    } = &held.entries()[0]
    else {
        panic!("expected an assistant entry");
    };
    // Aborted, not Error: the user stopped it, and nothing went wrong.
    assert_eq!(*stop_reason, Some(StopReason::Aborted));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(*held.status(), magi_proto::AgentStatus::Idle);

    drop(held);
}

#[tokio::test]
async fn a_compacted_session_sends_the_summary_and_not_the_history() {
    // What compaction is for. The point is not that a record exists; it is that the next
    // request is smaller and still says what the task was.
    let (session, _dir) = session("compacted-context");
    {
        let mut held = session.lock().await;
        for i in 0..12 {
            held.commit(Entry::User {
                aside: String::new(),
                id: magi_proto::MessageId::new(format!("u{i}")),
                text: format!("forgotten message {i}"),
            })
            .expect("commit");
        }
        held.commit(Entry::Compaction {
            id: magi_proto::MessageId::new("k1"),
            summary: "The user is porting a journal to Rust.".into(),
            replaces: 10,
        })
        .expect("commit");
        held.commit(Entry::User {
            id: magi_proto::MessageId::new("u99"),
            text: "carry on".into(),
            aside: String::new(),
        })
        .expect("commit");
    }

    let held = session.lock().await;
    let context = magi_host::context::of(&held);
    let sent = format!("{:?}", context.messages);
    assert!(sent.contains("porting a journal"), "the summary is sent");
    assert!(sent.contains("carry on"), "and what followed it");
    assert!(
        !sent.contains("forgotten message 0"),
        "but not what it replaced: {sent}"
    );
    // And the tail it deliberately kept. Starting from the compaction record rather than from
    // `replaces` threw this away — the recent turns are the whole reason the tail is kept.
    assert!(
        sent.contains("forgotten message 10") && sent.contains("forgotten message 11"),
        "the kept tail survives: {sent}"
    );
    drop(held);
}

#[tokio::test]
async fn the_wait_is_announced_while_it_is_happening() {
    // melchior does the waiting now, so a backoff is invisible from here and forty seconds of
    // nothing reads as a hang. The UI has had a `Retrying` display since M0; this is what fills
    // it in, and saying so after the fact would be no use to anybody watching a spinner.
    let (session, _dir) = session("announced");
    let mind = Mind::saying(
        "turn-announced",
        &[
            &retrying_line(1, 4, 0.5),
            &text_line("through in the end"),
            &stop_line(),
        ],
    );
    let mut live = session.lock().await.subscribe();
    turn(&session, &backend(&mind)).await;

    // Waited for rather than drained. `try_recv` in a loop asks what is in the channel at this
    // instant, and the publisher is another task: this test failed about one run in three
    // because the drain won the race and found nothing. A bounded wait is not a weaker
    // assertion — an event that never arrives still fails, it just no longer fails when the
    // event is merely late.
    let mut announced = None;
    let deadline = std::time::Duration::from_secs(5);
    while announced.is_none() {
        match tokio::time::timeout(deadline, live.recv()).await {
            Ok(Ok(magi_proto::HarnessEvent::StatusChanged {
                status:
                    magi_proto::AgentStatus::Retrying {
                        attempt, delay_ms, ..
                    },
                ..
            })) => announced = Some((attempt, delay_ms)),
            Ok(Ok(_)) => {}
            // Lagged means the buffer wrapped; the next read still returns the newer events.
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) | Err(_) => break,
        }
    }
    let (attempt, delay_ms) = announced.expect("the wait was published");
    assert_eq!(attempt, 1, "the first try is the one that failed");
    assert!(delay_ms > 0, "and it says how long");

    // And the turn still ended with the answer, not with the wait.
    let held = session.lock().await;
    let Entry::Assistant { text, .. } = &held.entries()[0] else {
        panic!("expected an assistant entry");
    };
    assert_eq!(text, "through in the end");
    drop(held);
    drop(live);
}
