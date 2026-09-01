//! The display under the box that says what the session is doing.
//!
//! A word said it before — `waiting` — and a word is a poor instrument for this. It is read
//! once and then never again, it says nothing about *how long*, and every state that is not
//! "working" looked identical to every other. What replaced it first was one character fading
//! down a ramp, which at the built-in frame interval had three steps to work with and came out
//! as a blink.
//!
//! So: braille. Nine cells are eighteen dot columns by four dot rows — a seventy-two pixel
//! display in the width of a short word. `axon.ui.beacon_cells` sets it, and the default is odd
//! so the shapes that are built around a middle have one to be built around.
//!
//! **Every column carries a heat as well as a shape**, and the cells are coloured from it. The
//! dots say where the energy is and the colour says how much: the core of a scanner is accent
//! and its fringes fall away to the border grey the prompt box is drawn in.
//!
//! **The scanner does not move at a constant rate.** One that crosses at one speed and turns
//! round instantly is a rectangle going back and forth; one that eases into the turn is a
//! machine. That is one cosine, in `swing`.
//!
//! Nothing here is a clock, either. Every animation is a phase from the frame counter and the
//! two settings that bracket it, so a state runs at the rate `axon.ui.beacon_ms` asks for
//! whatever the frame rate happens to be.

use crate::colour;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// Dot rows down the display.
const ROWS: usize = 4;

/// How many cells wide it is on a screen this wide.
///
/// `axon.ui.beacon_cells` asks for a width; this is the one it gets. The display is centred on
/// the row, and a centred thing lands on the exact middle only when the space left over either
/// side of it is the same -- which needs the display and the screen to be the same parity. Odd
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
    /// Nothing typed, nothing running. A slow wave, going nowhere in particular.
    Resting,
    /// Something is in the prompt and not sent yet. The wave picks up.
    Holding,
    /// A turn is running. A scanner crossing and coming back, easing into each turn.
    Working,
    /// A completion popup is open and typing narrows it. Two markers closing on the middle.
    Narrowing,
    /// A list or a permission is open and it is your move. A breath, in and out.
    Asking,
    /// The daemon is not there. A flat line with the signal dropping out of it.
    Away,
}

impl Mood {
    /// How long one cycle of this state's animation takes, as a multiple of the setting.
    ///
    /// Resting is slower than the setting because it is the state that should be possible to
    /// ignore. Working is *longer* than any of them, which is not the same as slower: its cycle
    /// is four crossings of the display, so one crossing is quicker than a resting wave even
    /// though the whole thing takes several times as long to come round.
    fn pace(self) -> (u64, u64) {
        match self {
            Self::Resting => (2, 1),
            Self::Holding => (1, 1),
            Self::Working => (5, 1),
            Self::Narrowing => (2, 3),
            Self::Asking => (3, 2),
            Self::Away => (2, 1),
        }
    }

    /// The colours this state burns through, coldest first.
    ///
    /// Three steps, and the cold end of most of them is the border grey the prompt box is drawn
    /// in — so the quiet parts of the display sit at the same level as the furniture and only
    /// what is happening reads as lit. The hot end is where the states differ, and it is what
    /// you can tell apart without looking straight at it.
    fn ramp(self) -> [Color; 3] {
        match self {
            Self::Resting => [colour::border(), colour::dim(), colour::muted()],
            Self::Holding => [colour::dim(), colour::muted(), colour::text()],
            Self::Working => [colour::border(), colour::muted(), colour::accent()],
            Self::Narrowing => [colour::border(), colour::dim(), colour::typed()],
            Self::Asking => [colour::border(), colour::dim(), colour::warning()],
            Self::Away => [colour::border(), colour::dim(), colour::error()],
        }
    }
}

mod shape;

use shape::draw;

/// The display as it stands this frame.
#[must_use]
pub fn render(mood: Mood, tick: usize, cells: usize) -> Vec<Span<'static>> {
    let shape = draw(mood, phase(mood, tick), cells * 2);
    let ramp = mood.ramp();
    (0..cells)
        .map(|cell| {
            // A cell is two columns and one colour, so it takes the hotter of the two rather
            // than their average: a scanner's core landing in the right half of a cell should
            // light that cell, not half-light it.
            let heat = shape.heat[cell * 2].max(shape.heat[cell * 2 + 1]);
            let step = (heat * ramp.len() as f32) as usize;
            Span::styled(
                cell_of(&shape.dots, cell).to_string(),
                Style::default().fg(ramp[step.min(ramp.len() - 1)]),
            )
        })
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
fn cell_of(dots: &[[bool; ROWS]], cell: usize) -> char {
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

/// Every state is symmetric about the middle, moves, is coloured by where its energy is, and is
/// always as many cells of braille as it was asked for.
#[cfg(test)]
mod tests {
    use super::*;

    const EVERY: [Mood; 6] = [
        Mood::Resting,
        Mood::Holding,
        Mood::Working,
        Mood::Narrowing,
        Mood::Asking,
        Mood::Away,
    ];

    /// Where in a cycle to sample. Enough to catch a state that is only wrong at one end.
    const STEPS: usize = 64;

    /// A width to draw at. Odd, like the built-in.
    const CELLS: usize = 9;

    /// And its dot COLUMNS.
    const COLUMNS: usize = CELLS * 2;

    /// The cells as one string.
    fn strip(mood: Mood, tick: usize) -> String {
        render(mood, tick, CELLS)
            .iter()
            .map(|s| s.content.to_string())
            .collect()
    }

    /// The colour of each cell.
    fn colours(mood: Mood, tick: usize) -> Vec<Option<Color>> {
        render(mood, tick, CELLS)
            .iter()
            .map(|s| s.style.fg)
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
    fn every_state_moves() {
        // A display that draws the same cells every frame is a picture, and the one thing this
        // has to do that a word could not is show that something is still running.
        for mood in EVERY {
            let first = strip(mood, 0);
            assert!(
                (1..STEPS).any(|tick| strip(mood, tick) != first),
                "{mood:?} never moved off {first:?}"
            );
        }
    }

    #[test]
    fn every_state_has_something_lit_at_every_moment() {
        // An empty frame reads as the UI having died, which is the opposite of what any of
        // these mean -- including `Away`, where the UI is the half that is still alive. The
        // scanner is the exception and the only one: it leaves the screen on purpose, waits,
        // and comes back, which is a different thing from never having been there.
        for mood in EVERY.into_iter().filter(|m| *m != Mood::Working) {
            for tick in 0..STEPS {
                let out = strip(mood, tick);
                assert!(out.chars().any(|c| c != '\u{2800}'), "{mood:?} at {tick}");
            }
        }
    }

    #[test]
    fn the_states_do_not_all_draw_the_same_thing() {
        // Told apart at a glance is the requirement. Two states that draw the same shape at the
        // same moment are one state with two names.
        for (index, mood) in EVERY.iter().enumerate() {
            for other in &EVERY[index + 1..] {
                assert!(
                    (0..STEPS).any(|tick| strip(*mood, tick) != strip(*other, tick)),
                    "{mood:?} and {other:?} draw the same thing throughout"
                );
            }
        }
    }

    #[test]
    fn working_and_asking_are_never_the_same_shape() {
        // The pair that matters most. One says keep waiting and the other says do something,
        // and a person reads this out of the corner of an eye.
        for step in 0..STEPS {
            let at = step as f32 / STEPS as f32;
            assert_ne!(
                draw(Mood::Working, at, COLUMNS).dots,
                draw(Mood::Asking, at, COLUMNS).dots,
                "at step {step} the scanner and the breath draw the same thing"
            );
        }
    }

    #[test]
    fn the_cells_are_not_all_one_colour() {
        // The whole reason a column carries a heat. One colour across the strip leaves the dots
        // doing all the work, and a scanner whose fringe is as bright as its core is a bar.
        for mood in EVERY {
            assert!(
                (0..STEPS).any(|tick| {
                    let seen = colours(mood, tick);
                    seen.iter().any(|c| *c != seen[0])
                }),
                "{mood:?} is one flat colour throughout"
            );
        }
    }

    #[test]
    fn every_state_reaches_the_hot_end_of_its_ramp() {
        // A ramp whose top step is never used is two colours with a third in the documentation.
        for mood in EVERY {
            let hottest = mood.ramp()[2];
            assert!(
                (0..STEPS).any(|tick| colours(mood, tick).contains(&Some(hottest))),
                "{mood:?} never reaches {hottest:?}"
            );
        }
    }

    #[test]
    fn narrowing_is_not_what_a_permission_ask_draws() {
        // `/` opening a menu drew the resting wave, which says nothing is happening -- and
        // something is. It must also not be mistaken for the one state that wants an answer.
        for step in 0..STEPS {
            let at = step as f32 / STEPS as f32;
            assert_ne!(
                draw(Mood::Narrowing, at, COLUMNS).dots,
                draw(Mood::Resting, at, COLUMNS).dots,
                "step {step}: a popup looks like an idle session"
            );
        }
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
