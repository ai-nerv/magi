//! The display under the box that says what the session is doing: braille cells drawn as a monitor,
//! scrolling right to left — a heartbeat while a turn runs, a flat line when nothing does, a square
//! wave while something on screen waits on you. `magi.ui.beacon_cells` sets the width, odd by
//! default so it lands on the exact middle of the row; see [`fitted`]. One colour, the footer's.
//! Nothing here is a clock: every animation is a phase from the frame counter and `magi.ui.beacon_ms`.

mod shape;

use crate::colour;
use ratatui::style::Style;
use ratatui::text::Span;
use shape::Dots;
pub use shape::Trace;

const ROWS: usize = 4;

/// How many cells wide it is on a screen this wide. A centred display lands on the exact middle
/// only when it and the screen share parity, so the asked-for width is moved up by one where it
/// has to be.
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

/// The display as it stands this frame. Winding the trace forward and drawing it are one call.
#[must_use]
pub fn render(trace: &mut Trace, mood: Mood, tick: usize, cells: usize) -> Vec<Span<'static>> {
    trace.advance(mood, tick, cells * 2);
    let dots = trace.dots(cells * 2);
    let style = Style::default().fg(colour::dim());
    (0..cells)
        .map(|cell| Span::styled(cell_of(&dots, cell).to_string(), style))
        .collect()
}

fn cell_of(dots: &Dots, cell: usize) -> char {
    // Braille numbers its dots 1-2-3-7 down the left and 4-5-6-8 down the right, which is not the
    // order of the bits: dots 7 and 8 took the two high bits above the original six.
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

    const STEPS: usize = 64;

    const CELLS: usize = 9;

    const EVERY: [Mood; 6] = [
        Mood::Resting,
        Mood::Holding,
        Mood::Working,
        Mood::Narrowing,
        Mood::Asking,
        Mood::Away,
    ];

    fn strip(mood: Mood, frames: usize) -> String {
        let mut trace = Trace::default();
        (0..frames)
            .map(|tick| {
                render(&mut trace, mood, tick, CELLS)
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .next_back()
            .unwrap_or_default()
    }

    #[test]
    fn it_is_always_the_configured_width_in_braille() {
        for mood in EVERY {
            let mut trace = Trace::default();
            for tick in 0..STEPS {
                let out: String = render(&mut trace, mood, tick, CELLS)
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect();
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
        let footer = Some(colour::dim());
        for mood in EVERY {
            let mut trace = Trace::default();
            for tick in 0..STEPS {
                for cell in render(&mut trace, mood, tick, CELLS) {
                    assert_eq!(cell.style.fg, footer, "{mood:?} at {tick}");
                }
            }
        }
    }

    #[test]
    fn a_running_turn_and_an_idle_one_are_told_apart() {
        // Checked through the packing rather than the dots: two shapes that pack the same are one.
        assert_ne!(
            strip(Mood::Working, STEPS),
            strip(Mood::Resting, STEPS),
            "a turn looks like an idle session"
        );
    }

    #[test]
    fn the_trace_carries_on_across_a_change_of_state() {
        // One tape at one speed. A per-state position computed from the frame counter teleports
        // the display when a turn ends.
        let mut trace = Trace::default();
        for tick in 0..STEPS {
            let _ = render(&mut trace, Mood::Working, tick, CELLS);
        }
        let beating: String = render(&mut trace, Mood::Working, STEPS, CELLS)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        let switched: String = render(&mut trace, Mood::Resting, STEPS, CELLS)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(
            beating, switched,
            "the display changed on a frame where the tape did not move"
        );
    }
}

/// The display lands on the exact middle of the row, at every terminal width.
#[cfg(test)]
mod centring_tests {
    use super::*;

    #[test]
    fn the_width_always_matches_the_screens_parity() {
        // Centring leaves `screen - cells` to split either side, and an odd split is half a column off.
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
