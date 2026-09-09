//! What crosses while a tool holds rows on the screen — the frames, and only the frames.
//!
//! Two enums, never one: [`ToSurface`] is what magi sends and [`FromSurface`] what comes back, so
//! the direction is in the type rather than in a comment somebody has to obey.

use crate::tooling::Span;
use serde::{Deserialize, Serialize};

/// What a key did. A terminal says this only when it speaks the Kitty keyboard protocol;
/// [`Self::Down`] is the default and is what every key looks like on one that cannot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Held {
    #[default]
    Down,
    Repeat,
    Up,
}

/// What the pointer did, wheel included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pointed {
    #[default]
    Press,
    Drag,
    Release,
    Moved,
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Middle,
    Right,
}

/// A cell, always in the surface's own coordinates: row 0, column 0 is its top-left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct At {
    pub row: u16,
    pub col: u16,
}

/// What magi sends a surface while it holds its rows. Frames rather than calls: one spawn lasts
/// the reservation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum ToSurface {
    /// The room it actually got, and what the call was given. The arguments travel with it so a
    /// surface opens knowing what it is about.
    Open {
        /// Rows granted, which may be fewer than were asked for.
        rows: u16,
        cols: u16,
        /// Whether this terminal reports key repeats and releases; `false` without the Kitty
        /// keyboard protocol, where a tenant waiting for a release is told there will never be one.
        #[serde(default)]
        holds: bool,
        #[serde(default)]
        args: serde_json::Value,
    },
    /// A key the person pressed while this surface held the rows. Named, not a scancode: magi has
    /// already decoded one to get here.
    Key {
        key: String,
        #[serde(default)]
        state: Held,
    },
    /// The pointer, over the rows this surface holds, in the surface's own coordinates; nothing
    /// landing outside the reservation is forwarded. magi never says where those rows are on
    /// screen, so it stays free to move them.
    Mouse {
        kind: Pointed,
        /// Which button, for the things a button does. Absent for motion and the wheel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<Button>,
        row: u16,
        col: u16,
    },
    Resize {
        rows: u16,
        cols: u16,
        /// Whether the keyboard reports holds, as currently known. Carried here as well as at open
        /// because it is learned: nothing proves the protocol is live until a repeat arrives.
        #[serde(default)]
        holds: bool,
    },
    /// Time passed. Only sent to a surface that named a [`crate::tooling::Surface::tick`].
    Tick,
    /// The reservation is over and nothing more will be read, so a tenant holding state can put it
    /// down rather than being killed mid-write.
    Close,
    /// What magi has to say about something the surface asked. Arrives out of band, between
    /// whatever frames the tenant was expecting.
    Answer {
        wondered: crate::wondering::Wondered,
        #[serde(flatten)]
        answered: crate::wondering::Answered,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum FromSurface {
    /// What to put in the rows. Clipped to the reservation, never grown by it: magi is the only
    /// one that knows what is below.
    Draw {
        lines: Vec<Vec<Span>>,
        /// Where the terminal's own cursor belongs, in this surface's coordinates. `None` leaves
        /// it in the prompt; a field somebody types into wants it here, because an IME candidate
        /// window and a screen reader both follow the real cursor.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<At>,
    },
    /// The surface is finished, and this is what the person chose: an id, never a decision. magi
    /// maps the id onto its own scopes and applies it.
    Done { answered: String },
    /// Something the surface would like to know. A closed list — see [`crate::wondering::Wonder`]
    /// — answered by a [`ToSurface::Answer`] naming the same question. Asking does not cost the
    /// surface its turn.
    Ask {
        wondered: crate::wondering::Wondered,
        /// What is being asked, by name, so a verb this magi never heard of arrives instead of
        /// failing to decode and is refused by name.
        wonder: String,
        #[serde(default)]
        args: serde_json::Value,
    },
}

#[cfg(test)]
mod frames {
    use super::*;
    use crate::tooling::{Role, Shown, Surface};

    #[test]
    fn a_surface_asks_for_room_and_says_what_it_is_for() {
        let asked = Shown::Surface(Surface {
            rows: 5,
            about: "a permission for `rm -rf build`".to_owned(),
            tick: None,
        });
        let wire = serde_json::to_string(&asked).expect("encodes");
        assert!(wire.contains(r#""shown":"surface""#), "{wire}");
        assert_eq!(
            serde_json::from_str::<Shown>(&wire).expect("decodes"),
            asked
        );
    }

    #[test]
    fn a_surface_can_say_what_was_chosen_and_not_what_it_means() {
        // It returns the id it drew, and magi decides; no field here could say "allowed".
        let done = FromSurface::Done {
            answered: "once".to_owned(),
        };
        let wire = serde_json::to_string(&done).expect("encodes");
        for granting in ["allow", "grant", "scope", "permit", "decision"] {
            assert!(!wire.contains(granting), "{granting} crossed: {wire}");
        }
    }

    #[test]
    fn what_magi_sends_and_what_comes_back_are_different_types() {
        let open = serde_json::to_string(&ToSurface::Open {
            rows: 5,
            cols: 92,
            holds: true,
            args: serde_json::Value::Null,
        })
        .expect("encodes");
        assert!(
            serde_json::from_str::<FromSurface>(&open).is_err(),
            "{open}"
        );
    }

    #[test]
    fn a_key_crosses_by_name_rather_than_by_scancode() {
        let wire = serde_json::to_string(&ToSurface::Key {
            key: "ctrl+c".to_owned(),
            state: Held::Down,
        })
        .expect("encodes");
        assert!(wire.contains(r#""key":"ctrl+c""#), "{wire}");
    }

    #[test]
    fn a_terminal_that_says_nothing_about_holding_says_down() {
        let plain: ToSurface =
            serde_json::from_str(r#"{"event":"key","key":"space"}"#).expect("decodes");
        assert_eq!(
            plain,
            ToSurface::Key {
                key: "space".to_owned(),
                state: Held::Down,
            }
        );
    }

    #[test]
    fn a_held_key_and_a_released_one_are_told_apart() {
        for (wire, state) in [
            (
                r#"{"event":"key","key":"space","state":"repeat"}"#,
                Held::Repeat,
            ),
            (r#"{"event":"key","key":"space","state":"up"}"#, Held::Up),
        ] {
            let read: ToSurface = serde_json::from_str(wire).expect("decodes");
            assert_eq!(
                read,
                ToSurface::Key {
                    key: "space".to_owned(),
                    state,
                }
            );
        }
    }

    #[test]
    fn a_click_crosses_in_the_surface_own_coordinates() {
        // Row zero is the tenant's first row, not the screen's: magi never says where the
        // reservation is, so it stays free to move it when the prompt grows a line.
        let wire = serde_json::to_string(&ToSurface::Mouse {
            kind: Pointed::Press,
            button: Some(Button::Left),
            row: 2,
            col: 11,
        })
        .expect("encodes");
        assert!(wire.contains(r#""row":2"#), "{wire}");
        assert!(wire.contains(r#""kind":"press""#), "{wire}");
        assert_eq!(
            serde_json::from_str::<ToSurface>(&wire).expect("decodes"),
            ToSurface::Mouse {
                kind: Pointed::Press,
                button: Some(Button::Left),
                row: 2,
                col: 11,
            }
        );
    }

    #[test]
    fn a_frame_that_wants_no_cursor_says_nothing_about_one() {
        let wire = serde_json::to_string(&FromSurface::Draw {
            lines: vec![vec![Span::new(Role::Text, "hi")]],
            cursor: None,
        })
        .expect("encodes");
        assert!(!wire.contains("cursor"), "{wire}");
    }

    #[test]
    fn a_tenant_that_wants_the_caret_says_where_in_its_own_rows() {
        let drew = FromSurface::Draw {
            lines: vec![vec![Span::new(Role::Text, "name: ")]],
            cursor: Some(At { row: 0, col: 6 }),
        };
        let wire = serde_json::to_string(&drew).expect("encodes");
        assert_eq!(
            serde_json::from_str::<FromSurface>(&wire).expect("decodes"),
            drew
        );
    }

    #[test]
    fn the_wheel_and_the_pointer_carry_no_button() {
        let wire = serde_json::to_string(&ToSurface::Mouse {
            kind: Pointed::ScrollDown,
            button: None,
            row: 0,
            col: 0,
        })
        .expect("encodes");
        assert!(!wire.contains("button"), "{wire}");
    }
}
