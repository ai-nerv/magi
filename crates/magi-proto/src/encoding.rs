//! What the protocol's own types encode to.
//!
//! Split out under THE RULE; the protocol next door is what these are about.

use super::*;

#[test]
fn cursor_advances() {
    assert_eq!(Cursor::ZERO.next(), Cursor(1));
}

#[test]
fn transport_errors_retry_and_auth_errors_do_not() {
    assert!(ErrorClass::Transport.is_retryable());
    assert!(ErrorClass::Overload.is_retryable());
    assert!(!ErrorClass::Auth.is_retryable());
    assert!(!ErrorClass::Invalid.is_retryable());
}

#[test]
fn every_event_reports_its_cursor() {
    let event = HarnessEvent::UserMessage {
        cursor: Cursor(7),
        id: MessageId::new("m1"),
        text: "hi".into(),
    };
    assert_eq!(event.cursor(), Cursor(7));
}

#[test]
fn envelope_stamps_the_current_version() {
    let envelope = Envelope::new(UiCommand::Interrupt);
    assert_eq!(envelope.version, PROTOCOL_VERSION);
}
