use super::*;
use magi_proto::{MessageId, StopReason};

/// A session holding nothing, which is what balthasar replays for one that has not run.
fn session(_name: &str) -> Session {
    Session::recorded(SessionId::new("s1"), Vec::new())
}

#[test]
fn a_revision_never_announces_an_ending_it_has_not_written() {
    // `revise` writes nothing, and `AssistantEnded` tells a listener the message is final —
    // `magi -p` acts on it. Asserted here, because in one process the `amend` wins the race.
    let mut session = session("revise-ending");
    let mut live = session.subscribe();
    let id = MessageId::new("a1");
    let started = Entry::Assistant {
        id: id.clone(),
        text: "half".to_owned(),
        thinking: String::new(),
        stop_reason: None,
        error: None,
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    };
    session.commit(started).expect("commit");
    let Entry::Assistant { text, .. } = session.entries()[0].clone() else {
        panic!("an assistant entry");
    };
    assert_eq!(text, "half");
    while live.try_recv().is_ok() {}

    // A finished message, revised rather than amended.
    session.revise(Entry::Assistant {
        id,
        text: "half an answer".to_owned(),
        thinking: String::new(),
        stop_reason: Some(StopReason::EndTurn),
        error: None,
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    });

    let mut sawticks = (false, false);
    while let Ok(event) = live.try_recv() {
        match event {
            HarnessEvent::AssistantDelta { .. } => sawticks.0 = true,
            HarnessEvent::AssistantEnded { .. } => sawticks.1 = true,
            _ => {}
        }
    }
    assert!(sawticks.0, "the growth is still published");
    assert!(
        !sawticks.1,
        "a revision must not announce an ending: nothing has been written"
    );
}

fn user(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(text),
        text: text.to_owned(),
        aside: String::new(),
    }
}

#[test]
fn committing_publishes_to_subscribers() {
    let mut s = session("publish");
    let mut rx = s.subscribe();
    s.commit(user("hi")).expect("commit");
    let event = rx.try_recv().expect("an event");
    assert!(matches!(event, HarnessEvent::UserMessage { .. }));
}

#[test]
fn a_cold_snapshot_carries_nothing() {
    let mut s = session("cold");
    s.commit(user("hi")).expect("commit");
    match s.snapshot(Cursor::ZERO) {
        HarnessEvent::SessionSnapshot { entries, .. } => assert!(entries.is_empty()),
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn a_resume_snapshot_carries_what_the_ui_already_saw() {
    let mut s = session("resume");
    s.commit(user("one")).expect("commit");
    s.commit(user("two")).expect("commit");
    match s.snapshot(Cursor(1)) {
        HarnessEvent::SessionSnapshot { entries, .. } => assert_eq!(entries.len(), 1),
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn replay_covers_only_what_follows_the_cursor() {
    let mut s = session("replay");
    s.commit(user("one")).expect("commit");
    s.commit(user("two")).expect("commit");
    let events = s.replay(Cursor(1));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cursor(), Cursor(2));
}

#[test]
fn an_unfinished_assistant_entry_replays_without_an_end_event() {
    let mut s = session("unfinished");
    s.commit(Entry::Assistant {
        id: MessageId::new("a1"),
        text: "partial".into(),
        thinking: String::new(),
        stop_reason: None,
        error: None,
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    })
    .expect("commit");
    let events = s.replay(Cursor::ZERO);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, HarnessEvent::AssistantEnded { .. })),
        "a turn still in flight has not ended"
    );
}

#[test]
fn a_finished_assistant_entry_replays_start_delta_and_end() {
    let mut s = session("finished");
    s.commit(Entry::Assistant {
        id: MessageId::new("a1"),
        text: "done".into(),
        thinking: String::new(),
        stop_reason: Some(StopReason::EndTurn),
        error: None,
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    })
    .expect("commit");
    assert_eq!(s.replay(Cursor::ZERO).len(), 3);
}

#[test]
fn status_is_published_but_not_journalled() {
    let mut s = session("status");
    let mut rx = s.subscribe();
    s.set_status(AgentStatus::Working {
        label: "Thinking".into(),
    });
    assert!(matches!(
        rx.try_recv().expect("an event"),
        HarnessEvent::StatusChanged { .. }
    ));
    assert!(
        s.entries().is_empty(),
        "status never reaches the transcript"
    );
}
