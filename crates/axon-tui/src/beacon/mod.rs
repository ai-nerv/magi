//! The display under the box that says what the session is doing.
//!
//! A word said it before — `waiting` — and a word is a poor instrument for this. It is read
//! once and then never again, it says nothing about *how long*, and every state that is not
//! "working" looked identical to every other.
//!
//! So: braille, drawn as a monitor. Nine cells are eighteen dot columns by four dot rows, which
//! is enough to draw a trace. It scrolls, continuously, right to left, and what runs through it
//! is a heartbeat while a turn is running, a flat line when nothing is, and a square wave while
//! something on screen is waiting on you. One instrument and a different signal on the wire — so
//! there is nothing to learn beyond what an ECG already taught everybody.
//!
//! `axon.ui.beacon_cells` sets the width, and the default is odd so the display lands on the
//! exact middle of the row. See [`fitted`].
//!
//! **One colour**, the same one the rest of the footer is written in. There were three per state
//! and they were doing work the trace does better: a heartbeat and a flat line are not two
//! shades of the same thing, they are two different pictures, and colouring them made the row
//! busy without making it clearer.
//!
//! Nothing here is a clock. Every animation is a phase from the frame counter and the two
//! settings that bracket it, so a state runs at the rate `axon.ui.beacon_ms` asks for whatever
//! the frame rate happens to be.

mod shape;

use crate::colour;
use ratatui::style::Style;
use ratatui::text::Span;
use shape::{Dots, draw};

/// Dot rows down the display.
const ROWS: usize = 4;

/// How many cells wide it is on a screen this wide.
///
/// `axon.ui.beacon_cells` asks for a width; this is the one it gets. The display is centred on
/// the row, and a centred thing lands on the exact middle only when the space left over either
/// side of it is the same — which needs the display and the screen to be the same parity. Odd
/// display on an even screen is half a column off centre, forever, and on a row whose other two
/// columns are pinned to the edges that is visible.
///
/// So the asked-for width is taken as a preference and moved by one where it has to be. Up
/// rather than down, because a display that grows by a cell reads as the same display and one
/// that shrinks below what was asked for reads as a bug.
#[must_use]
pub fn fitted(screen: u16) -> usize {
    let asked = usize::from(crate::metric::beacon_cells()).max(1);
    if usize::from(screen) % 2 == asked % 2 {
        asked
    } else {
        asked + 1
    }
}

/// What the session is doing, as the display draws it.
///
/// The same states the border's scan knows about, plus the two it does not: the daemon being
/// away, and something on screen waiting for an answer. Both were invisible here before, and
/// both are exactly when a person stares at the footer wondering why nothing is happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    /// Nothing typed, nothing running. A flat line.
    Resting,
    /// Something is in the prompt and not sent yet. Also a flat line: nothing is running.
    Holding,
    /// A turn is running. A heartbeat.
    Working,
    /// A completion popup is open and typing narrows it. A tight square wave.
    Narrowing,
    /// A list or a permission is open and it is your move. A square wave.
    Asking,
    /// The daemon is not there. A flat line with the lead off.
    Away,
}

impl Mood {
    /// How long the trace takes to scroll one whole signal past, as a multiple of the setting.
    ///
    /// For the heartbeat that is a pulse rate, since the signal is one beat: the built-in works
    /// out at a beat every two seconds. For the rest it is how fast the wave travels.
    fn pace(self) -> (u64, u64) {
        match self {
            Self::Resting | Self::Holding => (3, 1),
            Self::Working => (2, 1),
            Self::Narrowing => (1, 1),
            Self::Asking => (3, 2),
            Self::Away => (2, 1),
        }
    }
}

/// The display as it stands this frame.
#[must_use]
pub fn render(mood: Mood, tick: usize, cells: usize) -> Vec<Span<'static>> {
    let dots = draw(mood, phase(mood, tick), cells * 2);
    let style = Style::default().fg(colour::dim());
    (0..cells)
        .map(|cell| Span::styled(cell_of(&dots, cell).to_string(), style))
        .collect()
}

/// Where this state is in its cycle, from 0.0 to just under 1.0.
fn phase(mood: Mood, tick: usize) -> f32 {
    let (slower, faster) = mood.pace();
    let cycle = (crate::metric::beacon_ms() * slower / faster).max(1);
    let frame = crate::metric::frame_ms().max(1);
    let elapsed = (tick as u64).saturating_mul(frame) % cycle;
    elapsed as f32 / cycle as f32
}

/// One braille cell of the display.
fn cell_of(dots: &Dots, cell: usize) -> char {
    // Braille numbers its dots 1-2-3-7 down the left and 4-5-6-8 down the right, which is not
    // the order the bits are in: dots 7 and 8 were added under the original six and took the two
    // high bits. Reading it off a table is the only way this stays right.
    const LEFT: [u8; ROWS] = [0, 1, 2, 6];
    const RIGHT: [u8; ROWS] = [3, 4, 5, 7];
    let mut bits = 0u8;
    for row in 0..ROWS {
        if dots[cell * 2][row] {
            bits |= 1 << LEFT[row];
        }
        if dots[cell * 2 + 1][row] {
            bits |= 1 << RIGHT[row];
        }
    }
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or('⠀')
}

/// It is always the width it was asked for, in braille, in one colour.
#[cfg(test)]
mod tests {
    use super::*;

    /// Where in a cycle to sample.
    const STEPS: usize = 64;

    /// A width to draw at, in cells.
    const CELLS: usize = 9;

    const EVERY: [Mood; 6] = [
        Mood::Resting,
        Mood::Holding,
        Mood::Working,
        Mood::Narrowing,
        Mood::Asking,
        Mood::Away,
    ];

    /// The cells as one string.
    fn strip(mood: Mood, tick: usize) -> String {
        render(mood, tick, CELLS)
            .iter()
            .map(|s| s.content.to_string())
            .collect()
    }

    #[test]
    fn it_is_always_the_configured_width_in_braille() {
        for mood in EVERY {
            for tick in 0..STEPS {
                let out = strip(mood, tick);
                assert_eq!(out.chars().count(), CELLS, "{mood:?} at {tick}: {out:?}");
                assert!(
                    out.chars().all(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
                    "{mood:?} at {tick}: {out:?}"
                );
            }
        }
    }

    #[test]
    fn it_is_all_one_colour() {
        // Three per state was doing work the trace does better. A heartbeat and a flat line are
        // not two shades of the same thing, they are two different pictures.
        let footer = Some(colour::dim());
        for mood in EVERY {
            for tick in 0..STEPS {
                for cell in render(mood, tick, CELLS) {
                    assert_eq!(cell.style.fg, footer, "{mood:?} at {tick}");
                }
            }
        }
    }

    #[test]
    fn a_running_turn_and_an_idle_one_are_told_apart() {
        // The distinction the whole display exists for, checked through the packing rather than
        // against the dots: two shapes that differ but pack to the same cells are one shape.
        assert!(
            (0..STEPS).any(|tick| strip(Mood::Working, tick) != strip(Mood::Resting, tick)),
            "a turn looks like an idle session"
        );
    }

    #[test]
    fn a_waiting_display_is_slower_than_a_working_one() {
        // The one thing the pace has to say: nothing is happening, so this is ignorable.
        let cycle = |mood: Mood| {
            let (slower, faster) = mood.pace();
            slower as f32 / faster as f32
        };
        assert!(cycle(Mood::Resting) > cycle(Mood::Working));
    }
}

/// The display lands on the exact middle of the row, at every terminal width.
#[cfg(test)]
mod centring_tests {
    use super::*;

    #[test]
    fn the_width_always_matches_the_screens_parity() {
        // The whole condition. Centring leaves `screen - cells` to split either side, and a
        // split of an odd number is half a column off -- one side always wider than the other.
        for screen in 20..200u16 {
            assert_eq!(
                usize::from(screen) % 2,
                fitted(screen) % 2,
                "at width {screen} the display cannot sit on the middle"
            );
        }
    }

    #[test]
    fn it_is_never_more_than_a_cell_off_what_was_asked_for() {
        let asked = usize::from(crate::metric::beacon_cells()).max(1);
        for screen in 20..200u16 {
            let got = fitted(screen);
            assert!(
                got.abs_diff(asked) <= 1,
                "at width {screen} it asked for {asked} and got {got}"
            );
            assert!(got >= asked, "and it never comes out narrower than asked");
        }
    }
}
