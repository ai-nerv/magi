//! A whole turn, against a melchior that answers.
//!
//! No account and no network: a script on disk plays the part, so the path a real turn takes —
//! ask built, melchior spawned, answer read a line at a time, deltas folded, entry amended,
//! journal written — is exercised end to end.
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
        casper: None,
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
    let path = dir.join("s.jsonl");
    let session = Session::open(&path, SessionId::new("s"), "/tmp", 0).expect("session");
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
async fn a_turn_streams_into_the_journal() {
    let (session, dir) = session("ok");
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

    let held = session.lock().await;
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

    // The journal holds it too, not just the in-memory transcript.
    let source = std::fs::read_to_string(dir.join("s.jsonl")).expect("journal");
    assert!(source.contains("append-only"), "the turn reached the disk");

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
async fn an_overflow_is_compacted_and_the_turn_carries_on() {
    // The failure that ends a long session, and the one refusal magi acts on rather than only
    // reports. `Overflow` exists as a class of its own for exactly this: told "it failed, try
    // later", a broker would give up on a turn a summary would have fixed.
    let (session, _dir) = session("overflow");
    // The refusal, then the summary the compaction asks for, then the answer. Two arms: the
    // last stands for every ask after the first.
    let mind = Mind::turns(
        "turn-overflow",
        &[
            &[&failed_line(
                "prompt is too long: 300000 tokens > 200000 maximum",
                "overflow",
            )],
            &[&text_line("The journal is append-only."), &stop_line()],
        ],
    );

    // Long enough to have something to summarise; `covers` declines below that.
    {
        let mut held = session.lock().await;
        for i in 0..12 {
            held.commit(Entry::User {
                aside: String::new(),
                id: magi_proto::MessageId::new(format!("u{i}")),
                text: format!("message number {i}"),
            })
            .expect("commit");
        }
    }
    turn(&session, &backend(&mind)).await;

    let held = session.lock().await;
    let entries = held.entries();
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Compaction { .. })),
        "the conversation was compacted: {entries:?}"
    );
    let last = entries.last().expect("an entry");
    let Entry::Assistant { text, .. } = last else {
        panic!("expected the retried answer, got {last:?}");
    };
    assert_eq!(text, "The journal is append-only.");

    // The refusal stays in the transcript. A reader noticing the model forget something needs
    // to be able to see that this is why.
    assert!(
        entries.iter().any(|e| matches!(
            e,
            Entry::Assistant { error: Some(why), .. } if why.contains("too long")
        )),
        "{entries:?}"
    );
    drop(held);
}

#[tokio::test]
async fn a_second_overflow_is_not_compacted_again() {
    // A conversation that still will not fit after summarising is not one that is too long: it
    // is one whose kept tail alone overflows, and compacting the summary would spend another
    // request to fail the same way.
    let (session, _dir) = session("twice");
    // Refused, summarised successfully, and refused again. The last arm repeats, so a turn
    // that went round a second time would keep asking rather than stop.
    let overflow = failed_line("prompt is too long", "overflow");
    let summary = text_line("The user is porting a journal.");
    let stop = stop_line();
    let mind = Mind::turns(
        "turn-twice",
        &[&[&overflow], &[&summary, &stop], &[&overflow]],
    );
    {
        let mut held = session.lock().await;
        for i in 0..12 {
            held.commit(Entry::User {
                aside: String::new(),
                id: magi_proto::MessageId::new(format!("u{i}")),
                text: format!("message number {i}"),
            })
            .expect("commit");
        }
    }
    turn(&session, &backend(&mind)).await;

    // The refused round, the summary that was asked for, and the round that refused again.
    // A fourth ask would be the second compaction this guards against.
    assert_eq!(mind.asked(), 3, "it kept trying to compact");
    let held = session.lock().await;
    assert_eq!(
        held.entries()
            .iter()
            .filter(|e| matches!(e, Entry::Compaction { .. }))
            .count(),
        1,
        "summarised once"
    );
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
