//! Streaming: what a message publishes as it grows, and what it settles as.
//!
//! Split from [`super`] under THE RULE, which caps a file at 800 lines.

use super::*;
use magi_proto::{MessageId, Signatures, StopReason, Usage};

/// A session holding nothing, which is what balthasar replays for one that has not run.
fn session() -> Session {
    Session::recorded(SessionId::new("s1"), Vec::new())
}

fn assistant(text: &str) -> Entry {
    Entry::Assistant {
        id: MessageId::new("a1"),
        text: text.to_owned(),
        thinking: String::new(),
        stop_reason: None,
        error: None,
        signatures: Signatures::default(),
        usage: Usage::default(),
    }
}

/// Everything published since the receiver was made.
fn drain(events: &mut tokio::sync::broadcast::Receiver<HarnessEvent>) -> Vec<HarnessEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

#[test]
fn a_message_still_arriving_is_published_a_piece_at_a_time() {
    // The milestone: a three-hundred word answer was fourteen seconds of spinner and then
    // the whole text at once, because nothing left the daemon until the message was done.
    let mut s = session();
    s.commit(assistant("")).expect("commit");
    let mut events = s.subscribe();

    s.revise(assistant("Hello"));
    s.revise(assistant("Hello there"));

    let said: Vec<String> = drain(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            HarnessEvent::AssistantDelta { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(said, vec!["Hello", " there"], "each piece, once");
}

#[test]
fn a_revision_is_not_handed_to_the_store_until_the_message_ends() {
    // **The queue coalesces by cursor, and that is what makes streaming affordable.** This read
    // the property off a JSONL file once — "nothing was flushed until the message ended" — and
    // there is no file: balthasar is the store, and what is queued for it is `pending`. Revising
    // does queue, deliberately (see `Session::revise`), because a flush landing between an
    // entry's commit and its settling amendment would otherwise record the empty message it
    // started as. What must not happen is a *write per revision*: a thousand-token answer would
    // send the message a thousand times, each copy longer than the last.
    let mut s = session();
    s.commit(assistant("")).expect("commit");
    for word in ["He", "Hell", "Hello"] {
        s.revise(assistant(word));
    }

    assert!(
        matches!(s.entries().last(), Some(Entry::Assistant { text, .. }) if text == "Hello"),
        "a UI attaching now sees what has arrived"
    );
    s.amend(assistant("Hello")).expect("amend");

    let queued = s.take_pending();
    assert_eq!(
        queued.len(),
        1,
        "one commit and three revisions cost one write, not four: {queued:?}"
    );
    assert!(
        matches!(&queued[0].1, Entry::Assistant { text, .. } if text == "Hello"),
        "and the one write is the finished message, not the empty one it began as"
    );
}

#[test]
fn a_message_taken_back_is_described_in_full_rather_than_as_an_append() {
    // What a retry mid-answer needs. A delta is an append, so describing a retraction as one
    // would leave both copies on screen.
    let mut s = session();
    s.commit(assistant("")).expect("commit");
    s.revise(assistant("half an answer"));
    let mut events = s.subscribe();

    s.revise(assistant(""));

    let published = drain(&mut events);
    assert!(
        published
            .iter()
            .any(|e| matches!(e, HarnessEvent::AssistantStarted { .. })),
        "the message begins again: {published:?}"
    );
    let appended: Vec<String> = published
        .into_iter()
        .filter_map(|event| match event {
            HarnessEvent::AssistantDelta { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert!(
        appended.iter().all(String::is_empty),
        "and nothing is appended: {appended:?}"
    );
}

#[test]
fn an_ordinary_ending_is_still_one_delta_and_a_stop() {
    // The repair must be invisible when a message merely finishes.
    let mut s = session();
    s.commit(assistant("")).expect("commit");
    s.revise(assistant("done"));
    let mut events = s.subscribe();

    let mut ended = assistant("done");
    if let Entry::Assistant { stop_reason, .. } = &mut ended {
        *stop_reason = Some(StopReason::EndTurn);
    }
    s.amend(ended).expect("amend");

    let published = drain(&mut events);
    assert!(
        !published
            .iter()
            .any(|e| matches!(e, HarnessEvent::AssistantStarted { .. })),
        "nothing began again: {published:?}"
    );
    assert!(
        published
            .iter()
            .any(|e| matches!(e, HarnessEvent::AssistantEnded { .. })),
        "it ended: {published:?}"
    );
}

/// The bug a spawned balthasar exposed: a message flushed mid-stream came back empty.
///
/// `revise` is what a streaming message calls per delta batch, and it did not queue. A flush
/// landing between the entry's first commit and its settling amendment therefore recorded the
/// empty message it started as, and that was the version the transcript kept.
#[test]
fn a_revised_message_is_queued_as_it_stands_not_as_it_started() {
    let mut session = Session::recorded(SessionId::new("s1"), Vec::new());
    let growing = |text: &str| Entry::Assistant {
        id: MessageId::new("a1"),
        text: text.into(),
        thinking: String::new(),
        stop_reason: None,
        error: None,
        signatures: Signatures::default(),
        usage: Usage::default(),
    };

    session.commit(growing("")).expect("commit");
    session.revise(growing("par"));
    session.revise(growing("partial answer"));

    let queued = session.take_pending();
    assert_eq!(queued.len(), 1, "one cursor, one write: {queued:?}");
    assert_eq!(
        queued[0].1,
        growing("partial answer"),
        "the queue must hold the message as it stands"
    );
}
