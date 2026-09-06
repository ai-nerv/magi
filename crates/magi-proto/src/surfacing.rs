//! What crosses while a tool holds rows on the screen.
//!
//! The frames, and only the frames. [`crate::tooling::Surface`] is a tool *asking* for rows, which
//! is part of the contract every tool is written against; this is what magi and a tenant say to
//! each other once the rows exist, which nothing but the two of them ever sees.
//!
//! Two enums, never one. [`ToSurface`] is what magi sends and [`FromSurface`] is what comes back,
//! so a tenant cannot send `Draw` at magi and magi cannot send `Done` at a tenant — the direction
//! is in the type rather than in a comment somebody has to obey.

use crate::tooling::Span;
use serde::{Deserialize, Serialize};

/// What a key did.
///
/// **A terminal only says this when it speaks the Kitty keyboard protocol.** Without it there is
/// one indistinguishable press per repeat and no word at all when a key comes back up, so nothing
/// can tell "tapped" from "still holding" — which is the whole difference between a hop and a jump
/// in anything reading the keyboard as a control rather than as text.
///
/// [`Self::Down`] is the default, and is what every key looks like on a terminal that cannot say
/// more. A tenant that only reads `Down` behaves the same either way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Held {
    /// It went down.
    #[default]
    Down,
    /// It is still down, and the terminal is repeating it.
    Repeat,
    /// It came back up.
    Up,
}

/// What the pointer did.
///
/// The wheel is here rather than beside it because a surface that scrolls and a surface that is
/// clicked are the same surface, and a tenant reading one field decides what it cares about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pointed {
    /// A button went down.
    #[default]
    Press,
    /// The pointer moved with a button held.
    Drag,
    /// A button came back up.
    Release,
    /// The pointer moved with nothing held.
    Moved,
    /// The wheel went up.
    ScrollUp,
    /// The wheel went down.
    ScrollDown,
}

/// Which button.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    /// The one everything uses.
    #[default]
    Left,
    /// The middle button, which on most mice is the wheel.
    Middle,
    /// The right button.
    Right,
}

/// A cell, in the coordinates of whatever names it.
///
/// Always the surface's own: row 0, column 0 is its top-left. The same convention in both
/// directions, so a tenant told where a click landed can answer with where the cursor should go
/// without converting anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct At {
    /// Rows down from the surface's first row.
    pub row: u16,
    /// Columns across from the surface's first column.
    pub col: u16,
}

/// What magi sends a surface while it holds its rows.
///
/// Frames rather than calls: a surface redraws per keystroke, so the spawn lives for the length of
/// the reservation instead of one exec per event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum ToSurface {
    /// The room it actually got, and what the call was given.
    ///
    /// The arguments travel with it so a surface opens knowing what it is about — a permission
    /// needs the command, a picker needs the list — rather than being told on some later frame.
    Open {
        /// Rows granted, which may be fewer than were asked for.
        rows: u16,
        /// Columns granted.
        cols: u16,
        /// Whether this terminal reports key repeats and releases.
        ///
        /// `false` on a terminal without the Kitty keyboard protocol, where every key arrives as
        /// a bare press. A tenant that would wait for a release is told there will never be one.
        #[serde(default)]
        holds: bool,
        /// The call's arguments.
        #[serde(default)]
        args: serde_json::Value,
    },
    /// A key the person pressed while this surface held the rows.
    ///
    /// Named, not a scancode: a tenant should not have to know how this terminal encodes a
    /// keypress, and magi has already decoded one to get here.
    Key {
        /// `j`, `enter`, `esc`, `ctrl+c`.
        key: String,
        /// Whether it went down, repeated, or came back up.
        #[serde(default)]
        state: Held,
    },
    /// The pointer, somewhere over the rows this surface holds.
    ///
    /// **In the surface's own coordinates.** Row 0, column 0 is its top-left cell, and nothing
    /// landing outside the reservation is forwarded at all. magi never says where those rows are
    /// on screen: they move whenever the prompt grows a line, and a tenant that had been told
    /// would be one magi could no longer place freely.
    ///
    /// This is what makes the rows a *screen* rather than somewhere keys are echoed. A picker can
    /// be clicked, a diff can be scrolled, and neither had to be given the keyboard first.
    Mouse {
        /// What it did.
        kind: Pointed,
        /// Which button, for the things a button does. Absent for motion and the wheel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<Button>,
        /// Rows down from the surface's own first row.
        row: u16,
        /// Columns across from the surface's own first column.
        col: u16,
    },
    /// The room changed under it, because the window did.
    Resize {
        /// Rows now.
        rows: u16,
        /// Columns now.
        cols: u16,
        /// Whether the keyboard reports holds, as currently known.
        ///
        /// Carried here as well as at open because it is *learned*: nothing proves the protocol is
        /// live until a repeat or a release arrives, which may be after the surface opened. A
        /// tenant told only at open would offer the lesser control for the rest of its life.
        #[serde(default)]
        holds: bool,
    },
    /// Time passed, for a surface that asked for it.
    ///
    /// Only sent to one that named a [`crate::tooling::Surface::tick`]. A game needs the world to move while
    /// nobody is pressing anything, and a picker does not.
    Tick,
    /// The reservation is over and nothing more will be read.
    ///
    /// Sent when the turn is cancelled or the session ends, so a tenant holding state can put it
    /// down rather than being killed mid-write.
    Close,
    /// What magi has to say about something the surface asked.
    ///
    /// Arrives out of band, between whatever frames the tenant was expecting: it is a reply to a
    /// question, not a turn of the loop, so nothing about the keyboard or the clock is implied by
    /// one landing.
    Answer {
        /// Which question this belongs to.
        wondered: crate::wondering::Wondered,
        /// What magi said, or why it said nothing.
        #[serde(flatten)]
        answered: crate::wondering::Answered,
    },
}

/// What a surface sends back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum FromSurface {
    /// What to put in the rows, in the same roles everything else is painted in.
    ///
    /// Clipped to the reservation, never grown by it: a tenant that sent more rows than it was
    /// given would run over whatever is below it, and magi is the only one that knows what that
    /// is.
    Draw {
        /// Each row, as the spans it is made of.
        lines: Vec<Vec<Span>>,
        /// Where the terminal's own cursor belongs, in this surface's coordinates.
        ///
        /// `None` — almost always — leaves it in the prompt, where it was. A tenant that draws a
        /// field somebody types into wants it here instead: the block a surface paints itself is
        /// a picture of a cursor, and an IME candidate window and a screen reader both follow the
        /// real one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<At>,
    },
    /// The surface is finished, and this is what the person chose.
    ///
    /// An *id*, never a decision. A surface that returned "allowed" would be a sibling granting
    /// itself a permission, which is what the ledger exists to prevent — magi maps this id onto
    /// its own scopes and applies it.
    Done {
        /// The id of whatever was chosen, as the tool named it.
        answered: String,
    },
    /// Something the surface would like to know.
    ///
    /// **This is what makes a surface a participant rather than a screen.** Everything else here
    /// is a tenant being told; this is one asking. What it may ask is a closed list — see
    /// [`crate::wondering::Wonder`] — and the answer comes back as a [`ToSurface::Answer`]
    /// naming the same question.
    ///
    /// Asking does not cost the surface its turn. A tenant may ask and go on drawing, and the
    /// answer arrives whenever magi has one.
    Ask {
        /// This question, so the answer can be matched to it.
        wondered: crate::wondering::Wondered,
        /// What is being asked, by name.
        ///
        /// A name rather than a decoded [`crate::wondering::Wonder`], so that a verb this magi has
        /// never heard of arrives instead of failing to decode. It is refused, by name — and a
        /// tenant built against a newer magi is told so rather than left waiting.
        wonder: String,
        /// What the verb takes, where it takes anything.
        #[serde(default)]
        args: serde_json::Value,
    },
}

/// Rows a tool asks for, and what may cross while it holds them.
#[cfg(test)]
mod frames {
    use super::*;
    use crate::tooling::{Role, Shown, Surface};

    #[test]
    fn a_surface_asks_for_room_and_says_what_it_is_for() {
        // The second half is not decoration: `magi -p` has no screen, and a run that silently
        // drew nothing would look like a hang rather than like something declining.
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
        // The whole trust boundary in one assertion. There is no field here a tenant could set to
        // "allowed", "always" or any other scope: it returns the id it drew, and magi decides.
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
        // One enum for both directions would let a tenant send `Key` and magi send `Draw`, and
        // the first thing either did with the other's frame would be to ask what it was.
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
        // magi has already decoded a keypress to get here. Handing on the bytes would make every
        // tenant learn this terminal's encoding to read an `enter`.
        let wire = serde_json::to_string(&ToSurface::Key {
            key: "ctrl+c".to_owned(),
            state: Held::Down,
        })
        .expect("encodes");
        assert!(wire.contains(r#""key":"ctrl+c""#), "{wire}");
    }

    #[test]
    fn a_terminal_that_says_nothing_about_holding_says_down() {
        // Most of them, and every one before the Kitty protocol. A tenant that reads only `down`
        // behaves identically whether or not the terminal can say more.
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
        // The whole point of asking for event types: without them there is one indistinguishable
        // press per repeat and no word at all when a key comes back up.
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
        // Row zero is the tenant's first row, not the screen's. magi never says where the
        // reservation is, so a surface that had been told its own y would be one magi could no
        // longer move when the prompt grew a line.
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
        // Nearly every frame. A `"cursor":null` on each one would be a field every reader has to
        // look at to find out that no surface has ever used it.
        let wire = serde_json::to_string(&FromSurface::Draw {
            lines: vec![vec![Span::new(Role::Text, "hi")]],
            cursor: None,
        })
        .expect("encodes");
        assert!(!wire.contains("cursor"), "{wire}");
    }

    #[test]
    fn a_tenant_that_wants_the_caret_says_where_in_its_own_rows() {
        // The same coordinates a click arrives in, so a field can put the caret where the pointer
        // just landed without converting anything.
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
        // There is none. A default `left` on the wire would have a tenant reading a scroll as a
        // click somebody never made.
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
