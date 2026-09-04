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
///
/// **The whole keyboard, since a surface may be a program.** This was once six keys, on the
/// argument that a tenant wanting more was asking for a text editor. Then a tenant *was* one: a
/// surface can hold a pty now, and `htop` wants its function keys and `vim` wants everything. A
/// key with no name here is a key that program can never be sent.
#[must_use]
pub fn named(key: KeyEvent) -> Option<String> {
    let base = match key.code {
        KeyCode::Char(' ') => "space".to_owned(),
        // **As the terminal sent it, capital or not.** It used to be lowercased, so that a tenant
        // binding `j` caught the shifted one too. That silently made a capital letter untypeable,
        // which is fine for a game and not for anything you type into.
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Esc => "esc".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => "backtab".to_owned(),
        KeyCode::Backspace => "backspace".to_owned(),
        KeyCode::Delete => "delete".to_owned(),
        KeyCode::Insert => "insert".to_owned(),
        KeyCode::Home => "home".to_owned(),
        KeyCode::End => "end".to_owned(),
        KeyCode::PageUp => "pageup".to_owned(),
        KeyCode::PageDown => "pagedown".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };
    // Shift stays out of it: it is already in the character the terminal sent, and naming it as
    // well would give one keypress two names. On the keys that carry no character it is the only
    // way to say so, which is what `backtab` is instead of `shift+tab`.
    let mut name = base;
    if key.modifiers.contains(KeyModifiers::ALT) {
        name = format!("alt+{name}");
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        name = format!("ctrl+{name}");
    }
    Some(name)
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
        assert_eq!(named(key(KeyCode::Char('j'))).as_deref(), Some("j"));
    }

    #[test]
    fn control_and_alt_are_named_and_shift_is_not() {
        // Shift is already in the character the terminal sent. Naming it as well would give one
        // keypress two names, and a tenant binding `j` would see `shift+j` for a capital.
        let ctrl = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(named(ctrl).as_deref(), Some("ctrl+c"));
        let alt = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT);
        assert_eq!(named(alt).as_deref(), Some("alt+f"));
        let shift = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_eq!(named(shift).as_deref(), Some("J"));
    }

    #[test]
    fn a_capital_stays_a_capital() {
        // It used to be lowercased so a tenant binding `j` caught the shifted one too, which made
        // a capital letter untypeable — fine for a game, wrong for a pty with an editor in it.
        assert_eq!(named(key(KeyCode::Char('J'))).as_deref(), Some("J"));
    }

    #[test]
    fn the_keys_a_program_wants_have_names_too() {
        // `htop` wants its function keys and `vim` wants the lot. A key with no name here is one
        // that program can never be sent.
        assert_eq!(named(key(KeyCode::F(7))).as_deref(), Some("f7"));
        assert_eq!(named(key(KeyCode::Home)).as_deref(), Some("home"));
        assert_eq!(named(key(KeyCode::PageDown)).as_deref(), Some("pagedown"));
        assert_eq!(named(key(KeyCode::Delete)).as_deref(), Some("delete"));
        assert_eq!(named(key(KeyCode::BackTab)).as_deref(), Some("backtab"));
    }

    #[test]
    fn a_key_with_no_name_is_not_invented_one() {
        // A tenant matching on names should never have to guard against one nobody could have
        // anticipated. A key with no name here is a key nothing binds.
        assert_eq!(named(key(KeyCode::CapsLock)), None);
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
        // The one that makes a release arrive for a key that produces text — space, a letter.
        // Without it the protocol reports press and repeat for those and never an `up`.
        let _ = crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
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

/// What the pointer did, and with which button, or `None` for something a surface has no name for.
///
/// The middle and right buttons cross even though nothing in magi's own chrome uses them: a tenant
/// is a program with a screen, and deciding for it which buttons exist is the sort of narrowing
/// that has to be undone one tool at a time.
#[must_use]
pub fn pointed(
    kind: crossterm::event::MouseEventKind,
) -> Option<(
    magi_proto::tooling::Pointed,
    Option<magi_proto::tooling::Button>,
)> {
    use crossterm::event::{MouseButton, MouseEventKind};
    use magi_proto::tooling::{Button, Pointed};
    let button = |which| {
        Some(match which {
            MouseButton::Left => Button::Left,
            MouseButton::Middle => Button::Middle,
            MouseButton::Right => Button::Right,
        })
    };
    Some(match kind {
        MouseEventKind::Down(which) => (Pointed::Press, button(which)),
        MouseEventKind::Drag(which) => (Pointed::Drag, button(which)),
        MouseEventKind::Up(which) => (Pointed::Release, button(which)),
        MouseEventKind::Moved => (Pointed::Moved, None),
        MouseEventKind::ScrollUp => (Pointed::ScrollUp, None),
        MouseEventKind::ScrollDown => (Pointed::ScrollDown, None),
        // Horizontal scrolling, which almost nothing sends and nothing here reads.
        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => return None,
    })
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
    fn a_press_a_drag_and_a_release_stay_three_things() {
        // A tenant that could not tell them apart could not have a button you hold, which is what
        // both games use the pointer for.
        use crossterm::event::{MouseButton, MouseEventKind};
        use magi_proto::tooling::{Button, Pointed};
        assert_eq!(
            pointed(MouseEventKind::Down(MouseButton::Left)),
            Some((Pointed::Press, Some(Button::Left)))
        );
        assert_eq!(
            pointed(MouseEventKind::Up(MouseButton::Left)),
            Some((Pointed::Release, Some(Button::Left)))
        );
        assert_eq!(
            pointed(MouseEventKind::Drag(MouseButton::Right)),
            Some((Pointed::Drag, Some(Button::Right)))
        );
    }

    #[test]
    fn motion_and_the_wheel_have_no_button_to_report() {
        use crossterm::event::MouseEventKind;
        use magi_proto::tooling::Pointed;
        assert_eq!(pointed(MouseEventKind::Moved), Some((Pointed::Moved, None)));
        assert_eq!(
            pointed(MouseEventKind::ScrollDown),
            Some((Pointed::ScrollDown, None))
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
