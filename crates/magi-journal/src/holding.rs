//! Holding a transcript that belongs to somebody else.

use super::*;
use magi_proto::{MessageId, StopReason};

/// A user entry, said shortly.
fn user(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(text),
        text: text.to_owned(),
        aside: String::new(),
    }
}

/// An assistant entry, said shortly.
fn assistant(text: &str) -> Entry {
    Entry::Assistant {
        id: MessageId::new(text),
        text: text.to_owned(),
        thinking: String::new(),
        stop_reason: Some(StopReason::EndTurn),
        error: None,
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    }
}

/// A fresh session holds nothing and its first entry lands at cursor one.
#[test]
fn the_first_entry_is_at_cursor_one() {
    let mut held = Journal::recorded(SessionId::new("s"), Vec::new());
    assert!(held.entries().is_empty());
    assert_eq!(held.append(user("hello")).expect("appended"), Cursor(1));
    assert_eq!(held.cursor(), Cursor(1));
    assert_eq!(held.entries().len(), 1);
}

/// **What replaced the file.** A resumed session picks up balthasar's entries and numbers on from
/// the end of them, rather than starting again at one and writing over its own history.
#[test]
fn a_replayed_session_numbers_on_from_where_it_stopped() {
    let mut held = Journal::recorded(
        SessionId::new("s"),
        vec![user("one"), assistant("two"), user("three")],
    );
    assert_eq!(held.cursor(), Cursor(3));
    assert_eq!(held.append(assistant("four")).expect("appended"), Cursor(4));
    assert_eq!(held.entries().len(), 4);
}

/// A streaming message is replaced in place, and says nothing to anybody.
#[test]
fn revising_replaces_the_last_entry_without_growing_the_transcript() {
    let mut held = Journal::recorded(SessionId::new("s"), Vec::new());
    held.append(assistant("par")).expect("appended");
    held.revise(assistant("partial"));
    assert_eq!(held.entries().len(), 1);
    assert_eq!(held.cursor(), Cursor(1));
}

/// **The bug this method exists for.** A round of three tool calls commits three entries and then
/// answers them one at a time; amending only the *last* one put the first two results on the third
/// entry and then overwrote them, so two calls kept `result: null` for the rest of the session.
#[test]
fn an_earlier_entry_can_be_amended_without_touching_the_last() {
    let mut held = Journal::recorded(SessionId::new("s"), Vec::new());
    for name in ["a", "b", "c"] {
        held.append(user(name)).expect("appended");
    }
    held.amend_at(Cursor(1), user("answered")).expect("amended");
    let names: Vec<String> = held
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::User { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(names, ["answered", "b", "c"]);
    assert_eq!(held.cursor(), Cursor(3));
}

/// A cursor from somebody else's session names nothing here, and is ignored rather than refused.
#[test]
fn a_cursor_naming_nothing_is_ignored() {
    let mut held = Journal::recorded(SessionId::new("s"), vec![user("only")]);
    assert!(held.amend_at(Cursor(0), user("x")).is_ok());
    assert!(held.amend_at(Cursor(99), user("x")).is_ok());
    assert_eq!(held.entries().len(), 1);
}

/// Amending an empty transcript appends, rather than dropping the entry on the floor.
#[test]
fn amending_nothing_appends() {
    let mut held = Journal::recorded(SessionId::new("s"), Vec::new());
    assert_eq!(held.amend(user("first")).expect("amended"), Cursor(1));
    assert_eq!(held.entries().len(), 1);
}
