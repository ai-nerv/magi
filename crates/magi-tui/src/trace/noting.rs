//! What the trace keeps, and what it deliberately does not.
//!
//! Split out under THE RULE; the ring these fill is next door.

use super::*;
use magi_proto::{Cursor, MessageId, ToolCallId};

fn started(name: &str) -> HarnessEvent {
    HarnessEvent::ToolCallStarted {
        cursor: Cursor::ZERO,
        id: ToolCallId::new("t1".to_owned()),
        name: name.to_owned(),
        args: "{}".to_owned(),
    }
}

fn delta(text: &str) -> HarnessEvent {
    HarnessEvent::AssistantDelta {
        cursor: Cursor::ZERO,
        id: MessageId::new("a1".to_owned()),
        text: text.to_owned(),
        thinking: String::new(),
    }
}

#[test]
fn a_tool_call_is_one_row() {
    let mut trace = Trace::new();
    trace.note(&started("bash"));
    assert_eq!(trace.len(), 1);
    let drawn = trace.lines()[0].to_string();
    assert!(drawn.contains("tool"), "{drawn}");
    assert!(drawn.contains("bash"), "{drawn}");
}

#[test]
fn the_reply_itself_is_not_in_the_trace() {
    // **The rule that keeps this useful.** `AssistantDelta` arrives many times a second; a trace
    // that recorded it would be a transcript with worse formatting, and one reply would push
    // everything else out of the ring.
    let mut trace = Trace::new();
    for word in ["the", "quick", "brown", "fox"] {
        trace.note(&delta(word));
    }
    assert!(trace.is_empty(), "{} rows", trace.len());
}

#[test]
fn it_is_bounded_and_drops_the_oldest_first() {
    // A session is not bounded. A trace that grew with it would be a memory leak with a nice
    // name on it.
    let mut trace = Trace::new();
    for i in 0..KEEP + 50 {
        trace.note(&started(&format!("tool{i}")));
    }
    assert_eq!(trace.len(), KEEP);
    let first = trace.lines()[0].to_string();
    assert!(first.contains("tool50"), "the oldest 50 went: {first}");
}

#[test]
fn every_kind_has_a_chip_short_enough_for_the_column() {
    // The chip is a thing you learn by reading down the column, which stops working the moment
    // one of them is wider than the others' field.
    for kind in [
        Kind::Turn,
        Kind::Tool,
        Kind::Permit,
        Kind::Context,
        Kind::Model,
        Kind::Error,
    ] {
        assert!(
            !kind.chip().is_empty() && kind.chip().len() <= 5,
            "{:?} -> {:?}",
            kind,
            kind.chip()
        );
    }
}

#[test]
fn a_long_line_is_cut_on_a_character_rather_than_a_byte() {
    // A path with an accent in it, cut mid-scalar, renders as a replacement character.
    let mut trace = Trace::new();
    trace.note(&started(&"é".repeat(200)));
    let drawn = trace.lines()[0].to_string();
    assert!(!drawn.contains('\u{fffd}'), "{drawn}");
    assert!(drawn.contains('…'), "and it says it was cut: {drawn}");
}

#[test]
fn a_row_never_contains_a_newline() {
    // One event is one row. A message with a newline in it would otherwise silently become two,
    // and the second would have no chip.
    let mut trace = Trace::new();
    trace.note(&HarnessEvent::Refused {
        cursor: Cursor::ZERO,
        message: "no\nand here is why".to_owned(),
    });
    assert!(!trace.lines()[0].to_string().contains('\n'));
}
