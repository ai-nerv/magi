//! What a retry mid-answer leaves on screen.
//!
//! Split out under THE RULE, and its own file for its own reason: deltas reach the screen as
//! they arrive now, so an attempt that streams half an answer and then fails has already shown
//! it. These are about taking that back.

use super::*;

fn started() -> HarnessEvent {
    HarnessEvent::AssistantStarted {
        cursor: Cursor(1),
        id: MessageId::new("a1"),
    }
}

fn delta(text: &str) -> HarnessEvent {
    HarnessEvent::AssistantDelta {
        cursor: Cursor(2),
        id: MessageId::new("a1"),
        text: text.to_owned(),
        thinking: String::new(),
    }
}

fn app_with(events: Vec<HarnessEvent>) -> App {
    let mut app = App::new();
    for event in events {
        app.apply(event);
    }
    app
}

#[test]
fn a_retry_mid_answer_leaves_exactly_one_copy() {
    // The milestone's second half. Deltas reach the screen as they arrive now, so an
    // attempt that streams half an answer and then fails has already shown it. Beginning
    // the message again is how the daemon takes it back.
    let app = app_with(vec![
        started(),
        delta("half an ans"),
        started(),
        delta("the whole answer"),
    ]);
    assert_eq!(app.entries().len(), 1, "one message, not two");
    match &app.entries()[0] {
        Entry::Assistant { text, .. } => assert_eq!(text, "the whole answer"),
        other => panic!("expected an assistant entry, got {other:?}"),
    }
}

#[test]
fn beginning_again_clears_what_the_failed_attempt_ended_with() {
    // An attempt can fail after the model has already stopped, and a stop left in place
    // would render the retry's message as finished before it had said anything.
    let app = app_with(vec![
        started(),
        delta("half"),
        HarnessEvent::AssistantEnded {
            cursor: Cursor(3),
            id: MessageId::new("a1"),
            stop_reason: axon_proto::StopReason::Error,
            error: Some("overloaded".into()),
            usage: axon_proto::Usage::default(),
        },
        started(),
    ]);
    match &app.entries()[0] {
        Entry::Assistant {
            text,
            stop_reason,
            error,
            ..
        } => {
            assert!(text.is_empty());
            assert_eq!(*stop_reason, None, "and it is running again");
            assert_eq!(*error, None);
        }
        other => panic!("expected an assistant entry, got {other:?}"),
    }
}

#[test]
fn a_second_message_is_still_a_second_message() {
    // Only the *same* id begins again. A new one is a new message.
    let app = app_with(vec![
        started(),
        delta("first"),
        HarnessEvent::AssistantStarted {
            cursor: Cursor(4),
            id: MessageId::new("a2"),
        },
        HarnessEvent::AssistantDelta {
            cursor: Cursor(5),
            id: MessageId::new("a2"),
            text: "second".into(),
            thinking: String::new(),
        },
    ]);
    assert_eq!(app.entries().len(), 2);
}
