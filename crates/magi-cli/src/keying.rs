//! Naming a keypress, for whoever is holding the rows. A surface is drawn by another process, often
//! in another language, and handing it this terminal's bytes would make every tenant learn
//! crossterm's encoding to recognise an `enter`. magi has decoded one, so it passes the name.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What to call this keypress, or `None` for one nothing can name — a fallback string would make a
/// tenant guard against names it could not anticipate. The whole keyboard, because a surface may
/// hold a pty: a key with no name here is a key that program can never be sent.
#[must_use]
pub fn named(key: KeyEvent) -> Option<String> {
    let base = match key.code {
        KeyCode::Char(' ') => "space".to_owned(),
        // As the terminal sent it, capital or not: lowercasing makes a capital letter untypeable.
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
    // Shift stays out of it: it is already in the character the terminal sent. On keys carrying no
    // character it is the only way to say so, which is what `backtab` is instead of `shift+tab`.
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
        // Shift is already in the character the terminal sent; naming it gives one key two names.
        let ctrl = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(named(ctrl).as_deref(), Some("ctrl+c"));
        let alt = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT);
        assert_eq!(named(alt).as_deref(), Some("alt+f"));
        let shift = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_eq!(named(shift).as_deref(), Some("J"));
    }

    #[test]
    fn a_capital_stays_a_capital() {
        // Lowercasing made a capital untypeable — fine for a game, wrong for a pty with an editor.
        assert_eq!(named(key(KeyCode::Char('J'))).as_deref(), Some("J"));
    }

    #[test]
    fn the_keys_a_program_wants_have_names_too() {
        // A key with no name here is one that program can never be sent.
        assert_eq!(named(key(KeyCode::F(7))).as_deref(), Some("f7"));
        assert_eq!(named(key(KeyCode::Home)).as_deref(), Some("home"));
        assert_eq!(named(key(KeyCode::PageDown)).as_deref(), Some("pagedown"));
        assert_eq!(named(key(KeyCode::Delete)).as_deref(), Some("delete"));
        assert_eq!(named(key(KeyCode::BackTab)).as_deref(), Some("backtab"));
    }

    #[test]
    fn a_key_with_no_name_is_not_invented_one() {
        // A tenant matching on names should not have to guard against one nobody anticipated.
        assert_eq!(named(key(KeyCode::CapsLock)), None);
    }
}

/// Whether this terminal reports key releases, and can be asked to.
#[cfg(test)]
mod probe {
    #[test]
    fn the_enhancement_api_is_available() {
        // Compile-time only: asserts the symbols exist in the crossterm we build against.
        let _ = crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES;
        // The one that makes a release arrive for a key that produces text — space, a letter.
        let _ = crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        let _ = crossterm::event::KeyEventKind::Release;
        let _ = crossterm::event::KeyEventKind::Repeat;
    }
}

/// What this key event did: went down, repeated, or came back up. A terminal that does not speak
/// the Kitty protocol only sends presses, so everything is [`magi_proto::surfacing::Held::Down`].
#[must_use]
pub fn held(key: KeyEvent) -> magi_proto::surfacing::Held {
    use crossterm::event::KeyEventKind;
    use magi_proto::surfacing::Held;
    match key.kind {
        KeyEventKind::Press => Held::Down,
        KeyEventKind::Repeat => Held::Repeat,
        KeyEventKind::Release => Held::Up,
    }
}

/// What the pointer did, and with which button, or `None` for something a surface has no name for.
/// The middle and right buttons cross even though magi's own chrome uses neither.
#[must_use]
pub fn pointed(
    kind: crossterm::event::MouseEventKind,
) -> Option<(
    magi_proto::surfacing::Pointed,
    Option<magi_proto::surfacing::Button>,
)> {
    use crossterm::event::{MouseButton, MouseEventKind};
    use magi_proto::surfacing::{Button, Pointed};
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

/// What magi would forward for each kind of key event: if a `Repeat` arrives, this proves it
/// crosses as one. It says nothing about whether a given terminal ever sends one.
#[cfg(test)]
mod forwarding {
    use super::*;
    use magi_proto::surfacing::Held;

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
        // Holding a key would re-trigger on every repeat, so a jump lands and jumps again.
        use crossterm::event::KeyEventKind;
        assert_ne!(
            held(kind(KeyEventKind::Repeat)),
            held(kind(KeyEventKind::Press))
        );
    }

    #[test]
    fn a_press_a_drag_and_a_release_stay_three_things() {
        // A tenant that could not tell them apart could not have a button you hold.
        use crossterm::event::{MouseButton, MouseEventKind};
        use magi_proto::surfacing::{Button, Pointed};
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
        use magi_proto::surfacing::Pointed;
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
