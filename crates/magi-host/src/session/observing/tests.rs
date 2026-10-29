#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::*;
use crate::events::{Event, Receiver, decode, enable_local};
use crate::session::admission::Request;
use magi_proto::{MessageId, SessionId};

fn prompt() -> Request {
    Request::Opening(Entry::User {
        id: MessageId::new("private-prompt-id"),
        text: "private-prompt".into(),
        aside: "private-aside".into(),
        images: vec![],
    })
}

fn answer(stop: Option<StopReason>) -> Entry {
    Entry::Assistant {
        id: MessageId::new("private-answer-id"),
        text: "private-answer".into(),
        thinking: String::new(),
        stop_reason: stop,
        error: None,
        signatures: Default::default(),
        usage: Default::default(),
    }
}

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut events = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        assert!(
            !std::str::from_utf8(&bytes)
                .expect("JSON")
                .contains("private")
        );
        events.push(decode(&bytes).expect("event"));
    }
    events
}

fn outcomes(receiver: &Receiver) -> Vec<TurnOutcome> {
    drain(receiver)
        .iter()
        .map(|event| match event.fact {
            Fact::LogicalTurn { outcome, .. } => outcome,
            _ => panic!("unexpected event"),
        })
        .collect()
}

#[test]
fn terminal_classification_uses_final_evidence_and_preserves_uncertainty() {
    assert_eq!(terminal(None, false, false), TurnOutcome::Unknown);
    assert_eq!(terminal(None, true, false), TurnOutcome::Failed);
    assert_eq!(terminal(None, false, true), TurnOutcome::Cancelled);
    for stop in [StopReason::EndTurn, StopReason::Length] {
        assert_eq!(
            terminal(Some(&answer(Some(stop))), false, true),
            TurnOutcome::Completed
        );
        assert_eq!(
            terminal(Some(&answer(Some(stop))), true, false),
            TurnOutcome::Failed
        );
    }
    assert_eq!(
        terminal(Some(&answer(Some(StopReason::Error))), false, false),
        TurnOutcome::Failed
    );
    assert_eq!(
        terminal(Some(&answer(Some(StopReason::Aborted))), false, false),
        TurnOutcome::Cancelled
    );
    assert_eq!(
        terminal(Some(&answer(Some(StopReason::ToolUse))), false, false),
        TurnOutcome::Unknown
    );
    assert_eq!(
        terminal(Some(&answer(Some(StopReason::ToolUse))), false, true),
        TurnOutcome::Cancelled
    );
}

#[test]
fn native_admission_keeps_queue_identity_and_drop_outcomes() {
    if std::env::var_os("MAGI_EVENT_ADMISSION_PROBE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "session::observing::tests::native_admission_keeps_queue_identity_and_drop_outcomes"])
            .env("MAGI_EVENT_ADMISSION_PROBE", "1").output().expect("probe");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("test result: ok. 1 passed; 0 failed; 0 ignored;")
        );
        return;
    }
    let receiver = enable_local().expect("activation");
    let mut held = Session::recorded(SessionId::new("private-session"), vec![]);
    let began = held.cursor();
    let (owner, _) = held.admit(prompt()).expect("admit").expect("active");
    let admitted = drain(&receiver);
    assert_eq!(admitted.len(), 2);
    let first = admitted[0].correlation.turn_id;
    assert!(first.is_some());
    assert_eq!(held.event_turn(), first);
    assert_eq!(admitted[1].correlation.turn_id, first);
    assert!(held.admit(prompt()).expect("queue").is_none());
    let queued = drain(&receiver);
    assert_eq!(queued.len(), 2);
    let second = queued[0].correlation.turn_id;
    assert_ne!(first, second);
    assert_eq!(held.event_turn(), first);
    assert_eq!(queued[1].correlation.turn_id, second);
    assert!(held.admit(prompt().tracked(Some(777))).is_err());
    assert_eq!(outcomes(&receiver), [TurnOutcome::Refused]);
    held.observe_finished(owner + 1, began, true);
    assert!(held.finish(owner + 1).is_none());
    assert!(drain(&receiver).is_empty());
    let Request::Opening(entry) = prompt() else {
        unreachable!()
    };
    held.commit(entry).expect("prompt");
    held.commit(answer(Some(StopReason::EndTurn)))
        .expect("answer");
    held.observe_finished(owner, began, false);
    let (next, _) = held.finish(owner).expect("next activation");
    assert_eq!(held.event_turn(), second);
    let finished = drain(&receiver);
    assert_eq!(finished.len(), 2);
    assert_eq!(finished[0].correlation.turn_id, first);
    assert_eq!(finished[1].correlation.turn_id, second);
    assert!(matches!(
        finished[0].fact,
        Fact::LogicalTurn {
            outcome: TurnOutcome::Completed,
            duration_ms: Some(_)
        }
    ));
    assert!(matches!(
        finished[1].fact,
        Fact::LogicalTurn {
            outcome: TurnOutcome::Started,
            duration_ms: None
        }
    ));
    assert!(held.finish(owner).is_none());
    assert!(drain(&receiver).is_empty());
    assert!(held.admit(prompt()).expect("queued").is_none());
    assert_eq!(
        outcomes(&receiver),
        [TurnOutcome::Admitted, TurnOutcome::Queued]
    );
    drop(held);
    let dropped = outcomes(&receiver);
    assert_eq!(dropped.len(), 2);
    assert!(dropped.contains(&TurnOutcome::Unknown));
    assert!(dropped.contains(&TurnOutcome::Cancelled));
    assert!(next > owner);

    let mut resumed = Session::recorded(SessionId::new("private-resume"), vec![]);
    for request in [Request::Declare, Request::Grants(vec![]), Request::Reports] {
        let (owner, _) = resumed
            .admit(request)
            .expect("internal request")
            .expect("activation");
        assert!(resumed.finish(owner).is_none());
        assert_eq!(resumed.event_turn(), None);
    }
    assert!(drain(&receiver).is_empty());
}
