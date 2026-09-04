//! Naming a keypress, for whoever is holding the rows.
//!
//! A surface is drawn by another process, often in another language, and handing it the bytes this
//! terminal happened to send would make every tenant learn crossterm's encoding to recognise an
//! `enter`. magi has already decoded one to get here, so it passes on the name.
//!
//! Deliberately small. `j`, `space`, `enter`, `esc`, `up`, `ctrl+c` — the keys a person presses at
//! something on a screen. A tenant that needs more than this is asking for a text editor, and a
//! text editor is what the prompt box already is.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What to call this keypress, or `None` for one nothing can name.
///
/// `None` rather than a fallback string: a tenant matching on names should never have to guard
/// against one it could not have anticipated, and a key with no name is one nobody binds.
#[must_use]
pub fn named(key: KeyEvent) -> Option<String> {
    let base = match key.code {
        KeyCode::Char(' ') => "space".to_owned(),
        KeyCode::Char(c) => c.to_lowercase().to_string(),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Esc => "esc".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::Backspace => "backspace".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        _ => return None,
    };
    // Only control. Shift is already in the character the terminal sent, and naming it separately
    // would give two names for one keypress; alt is a meta key this has no use for yet.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(format!("ctrl+{base}"));
    }
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_keys_a_person_presses_at_something_have_names() {
        assert_eq!(named(key(KeyCode::Char(' '))).as_deref(), Some("space"));
        assert_eq!(named(key(KeyCode::Enter)).as_deref(), Some("enter"));
        assert_eq!(named(key(KeyCode::Esc)).as_deref(), Some("esc"));
        assert_eq!(named(key(KeyCode::Up)).as_deref(), Some("up"));
        assert_eq!(named(key(KeyCode::Char('J'))).as_deref(), Some("j"));
    }

    #[test]
    fn control_is_named_and_shift_is_not() {
        // Shift is already in the character the terminal sent. Naming it as well would give one
        // keypress two names, and a tenant binding `j` would miss the one that arrived as `J`.
        let ctrl = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(named(ctrl).as_deref(), Some("ctrl+c"));
        let shift = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::SHIFT);
        assert_eq!(named(shift).as_deref(), Some("j"));
    }

    #[test]
    fn a_key_with_no_name_is_not_invented_one() {
        // A tenant matching on names should never have to guard against one nobody could have
        // anticipated. A key with no name here is a key nothing binds.
        assert_eq!(named(key(KeyCode::F(7))), None);
    }
}

/// Whether this terminal reports key releases, and can be asked to.
#[cfg(test)]
mod probe {
    #[test]
    fn the_enhancement_api_is_available() {
        // Compile-time only: this asserts the symbols exist in the crossterm we build against,
        // so the release-reporting path below is not written against an API that is not there.
        let _ = crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        let _ = crossterm::event::KeyEventKind::Release;
        let _ = crossterm::event::KeyEventKind::Repeat;
    }
}

/// What this key event did: went down, repeated, or came back up.
///
/// A terminal that does not speak the Kitty protocol only ever sends presses, so everything is
/// [`magi_proto::tooling::Held::Down`] there — which is what a tenant reading only `down`
/// already expects.
#[must_use]
pub fn held(key: KeyEvent) -> magi_proto::tooling::Held {
    use crossterm::event::KeyEventKind;
    use magi_proto::tooling::Held;
    match key.kind {
        KeyEventKind::Press => Held::Down,
        KeyEventKind::Repeat => Held::Repeat,
        KeyEventKind::Release => Held::Up,
    }
}

/// What magi would forward for each kind of key event.
///
/// Isolates magi's own plumbing from the terminal's: if a `Repeat` arrives, this proves it
/// crosses as one. It says nothing about whether a given terminal ever *sends* one.
#[cfg(test)]
mod forwarding {
    use super::*;
    use magi_proto::tooling::Held;

    fn kind(kind: crossterm::event::KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::Char(' '), KeyModifiers::NONE, kind)
    }

    #[test]
    fn each_kind_crosses_as_itself() {
        use crossterm::event::KeyEventKind;
        assert_eq!(held(kind(KeyEventKind::Press)), Held::Down);
        assert_eq!(held(kind(KeyEventKind::Repeat)), Held::Repeat);
        assert_eq!(held(kind(KeyEventKind::Release)), Held::Up);
    }

    #[test]
    fn a_repeat_is_not_a_press() {
        // The symptom when this is wrong: holding a key re-triggers on every repeat, so a jump
        // lands and immediately jumps again. Distinguishable only if the two stay distinct all
        // the way across.
        use crossterm::event::KeyEventKind;
        assert_ne!(
            held(kind(KeyEventKind::Repeat)),
            held(kind(KeyEventKind::Press))
        );
    }

    #[test]
    fn every_kind_still_names_its_key() {
        // A release that lost its name would be a release nothing could act on.
        use crossterm::event::KeyEventKind;
        for one in [
            KeyEventKind::Press,
            KeyEventKind::Repeat,
            KeyEventKind::Release,
        ] {
            assert_eq!(named(kind(one)).as_deref(), Some("space"), "{one:?}");
        }
    }
}
