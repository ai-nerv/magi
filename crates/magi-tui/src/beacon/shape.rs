//! What each state puts on the wire, and the tape it is written onto. The trace scrolls the way an
//! ECG does: new samples at the right, the line running off to the left, always the full width.
//! One tape at one speed forever, so a state change only alters what is written onto its right-hand
//! end and the old signal scrolls off in its own time instead of the picture being replaced.

use super::{Mood, ROWS};

/// Lit dots, by column and then row. Row zero is the top.
pub(super) type Dots = Vec<[bool; ROWS]>;

/// A sample with nothing on the wire: not a height, and it lights nothing. Kept in the same table
/// as the heights so the gap scrolls along with everything else.
const GAP: u8 = u8::MAX;

/// The height everything rests at.
const LINE: u8 = 1;

/// One heartbeat, as a height per sample: zero is the bottom row and three the top. The rest at the
/// end is what makes it a pulse, and the table is nearly twice the display's width and mostly flat.
const HEARTBEAT: [u8; 32] = [
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // the line
    2, 1, // P
    0, 3, 3, 0, // QRS
    1, 2, 2, 1, // T
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // and back to the line
];

const FLAT: [u8; 1] = [LINE];

/// A square wave: something is waiting on an answer. Square because a prompt should look manufactured.
const SQUARE: [u8; 8] = [3, 3, 3, 3, 0, 0, 0, 0];

/// The same, tighter: half the period of [`SQUARE`], so twice as many cycles fit the display.
const CHOPPY: [u8; 4] = [3, 3, 0, 0];

/// The line dropping out: there is no daemon. Gaps arriving and scrolling past are what say this
/// end is still running, where a dead line would look like a hung display.
const LEADOFF: [u8; 10] = [LINE, LINE, LINE, GAP, GAP, LINE, LINE, LINE, GAP, LINE];

fn signal(mood: Mood) -> &'static [u8] {
    match mood {
        Mood::Working => &HEARTBEAT,
        Mood::Asking => &SQUARE,
        Mood::Narrowing => &CHOPPY,
        Mood::Away => &LEADOFF,
        Mood::Resting | Mood::Holding => &FLAT,
    }
}

/// The tape the trace is written on, newest last. State, deliberately: what is on the left of the
/// display is what the session was doing a second ago, which no arithmetic over the current state
/// recovers.
#[derive(Debug, Default)]
pub struct Trace {
    /// Samples, oldest first. Never kept longer than the widest display asked for.
    written: Vec<u8>,
    at: usize,
    was: Option<Mood>,
    since: usize,
    /// Fractions of a sample owed but not yet written.
    owed: f32,
}

impl Trace {
    /// Write however many samples have arrived since the last frame. Driven from the frame counter,
    /// so a UI that redraws slowly draws a slower trace rather than skipping most of it.
    pub fn advance(&mut self, mood: Mood, tick: usize, columns: usize) {
        // A new signal is written from its own beginning, so a heartbeat starts at the baseline.
        if self.was != Some(mood) {
            self.was = Some(mood);
            self.at = 0;
        }
        let frames = tick.saturating_sub(self.since);
        self.since = tick;
        self.owed += frames as f32 * rate(columns);
        let signal = signal(mood);
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            self.written.push(signal[self.at % signal.len()]);
            self.at += 1;
        }
        // Held to twice the width rather than exactly it, so the trim is occasional.
        if self.written.len() > columns * 2 {
            self.written.drain(..self.written.len() - columns);
        }
    }

    /// The trace as it stands, `columns` wide. Joined vertically between neighbouring samples, or
    /// the R spike is a dot floating above a line. A gap joins to nothing on either side.
    pub(super) fn dots(&self, columns: usize) -> Dots {
        let mut dots = vec![[false; ROWS]; columns];
        // Short of a full display, the rest is baseline rather than half-drawn.
        let short = columns.saturating_sub(self.written.len());
        let at = |x: usize| {
            if x < short {
                return LINE;
            }
            // Counted back from the newest sample, so the right-hand edge is always what just arrived.
            self.written[self.written.len() - (columns - x)]
        };
        for (x, column) in dots.iter_mut().enumerate() {
            let here = at(x);
            if here == GAP {
                continue;
            }
            let before = if x == 0 { here } else { at(x - 1) };
            let joined = if before == GAP { here } else { before };
            for height in here.min(joined)..=here.max(joined) {
                column[ROWS - 1 - usize::from(height).min(ROWS - 1)] = true;
            }
        }
        dots
    }
}

/// How many samples arrive per frame, so the trace crosses the display in `magi.ui.beacon_ms`. One
/// rate for every state: two scroll positions from the same frame counter are two pictures.
fn rate(columns: usize) -> f32 {
    let across = crate::metric::beacon_ms().max(1) as f32;
    columns as f32 * crate::metric::frame_ms().max(1) as f32 / across
}

/// One tape, one speed, and a different signal written onto it for each state.
#[cfg(test)]
mod tests {
    use super::*;

    const COLUMNS: usize = 18;

    fn wound(mood: Mood, frames: usize) -> Trace {
        let mut trace = Trace::default();
        for tick in 0..frames {
            trace.advance(mood, tick, COLUMNS);
        }
        trace
    }

    fn rows(dots: &Dots) -> Vec<Vec<usize>> {
        dots.iter()
            .map(|column| (0..ROWS).filter(|row| column[*row]).collect())
            .collect()
    }

    #[test]
    fn a_fresh_trace_is_a_flat_line() {
        let dots = Trace::default().dots(COLUMNS);
        for (x, on) in rows(&dots).iter().enumerate() {
            assert_eq!(on, &vec![ROWS - 2], "column {x} is not on the line");
        }
    }

    #[test]
    fn the_trace_scrolls() {
        let mut trace = wound(Mood::Working, 40);
        let before = trace.dots(COLUMNS);
        for tick in 40..64 {
            trace.advance(Mood::Working, tick, COLUMNS);
        }
        assert_ne!(before, trace.dots(COLUMNS), "it never moved");
    }

    #[test]
    fn a_change_of_state_writes_nothing_on_its_own() {
        // Switching signals within one frame must not move the trace: what is on screen happened.
        let mut trace = wound(Mood::Working, 200);
        let before = trace.dots(COLUMNS);
        // The same frame the winding ended on, so no time has passed.
        trace.advance(Mood::Resting, 199, COLUMNS);
        assert_eq!(
            before,
            trace.dots(COLUMNS),
            "the trace moved on a frame where no time passed"
        );
    }

    #[test]
    fn a_change_of_state_scrolls_rather_than_cutting() {
        // With time passing, the display that follows is the one before it shifted along.
        let mut trace = wound(Mood::Working, 200);
        let before = trace.dots(COLUMNS);
        trace.advance(Mood::Resting, 200, COLUMNS);
        let after = trace.dots(COLUMNS);
        // From the second column in. The leftmost one lost the neighbour it was joined to when that
        // neighbour scrolled off, so it is the join that changed, not the sample.
        let kept = COLUMNS - 6;
        let shifted = (1..=4).any(|by| before[by + 1..by + 1 + kept] == after[1..1 + kept]);
        assert!(
            shifted,
            "the display is not the one before it moved along:\n{before:?}\n{after:?}"
        );
    }

    #[test]
    fn a_new_signal_arrives_from_the_right() {
        // And having not jumped, it has to change -- by scrolling in, one sample at a time.
        let mut trace = wound(Mood::Working, 200);
        let beating = trace.dots(COLUMNS);
        for tick in 200..260 {
            trace.advance(Mood::Resting, tick, COLUMNS);
        }
        let flat = trace.dots(COLUMNS);
        assert_ne!(beating, flat, "the flat line never arrived");
        for (x, on) in rows(&flat).iter().enumerate() {
            assert_eq!(on, &vec![ROWS - 2], "column {x} is still not flat");
        }
    }

    #[test]
    fn a_signal_starts_from_its_own_beginning() {
        // A heartbeat that starts mid-spike is a glitch arriving, not a beat.
        let mut trace = wound(Mood::Asking, 200);
        // Measured off `at`, which counts what this signal has written: the tape is trimmed as it
        // grows and any index into it goes stale. One advance first, or a loop guarded on `at`
        // never runs, because until the new signal is written once `at` is still the old count.
        let mut tick = 200;
        trace.advance(Mood::Working, tick, COLUMNS);
        while trace.at < 4 && tick < 400 {
            tick += 1;
            trace.advance(Mood::Working, tick, COLUMNS);
        }
        assert!(trace.at >= 4, "nothing was written in two hundred frames");
        let since = &trace.written[trace.written.len() - trace.at..];
        assert_eq!(
            since,
            &HEARTBEAT[..trace.at],
            "the beat did not start at the start of the beat"
        );
    }

    #[test]
    fn the_heartbeat_has_a_spike_and_a_rest() {
        // Every heartbeat there has ever been. A waveform with no pause in it is a signal.
        let top = HEARTBEAT.iter().filter(|h| **h == 3).count();
        assert!(top > 0, "there is an R wave");
        assert!(
            top * 4 < HEARTBEAT.len(),
            "and it is a spike, not a plateau: {top} of {}",
            HEARTBEAT.len()
        );
        let resting = HEARTBEAT.iter().filter(|h| **h == LINE).count();
        assert!(
            resting * 2 > HEARTBEAT.len(),
            "and it spends most of the beat at rest"
        );
    }

    #[test]
    fn the_trace_is_drawn_joined() {
        // The R wave climbs three rows in one sample; unjoined that is a speck of dust, not a beat.
        let dots = wound(Mood::Working, 400).dots(COLUMNS);
        for (x, on) in rows(&dots).iter().enumerate() {
            assert!(!on.is_empty(), "column {x} is empty");
            assert_eq!(
                on.last().expect("lit") - on[0] + 1,
                on.len(),
                "column {x} has a hole in it: {on:?}"
            );
        }
    }

    #[test]
    fn the_lead_off_line_has_gaps_in_it() {
        // The gaps arriving are what say this end is still running while the other is not.
        let dots = wound(Mood::Away, 400).dots(COLUMNS);
        assert!(
            dots.iter().any(|column| column.iter().all(|on| !on)),
            "there is no break in the line"
        );
    }

    #[test]
    fn a_menu_and_a_permission_are_not_the_same_square() {
        // Both wait on you, for different things. Counted as edges: a table is a period and the
        // width sets the frequency.
        let edges = |mood: Mood| {
            let dots = wound(mood, 400).dots(COLUMNS);
            (1..COLUMNS).filter(|x| dots[*x] != dots[x - 1]).count()
        };
        assert!(
            edges(Mood::Narrowing) > edges(Mood::Asking),
            "a narrowing menu is not busier than a permission ask: {} against {}",
            edges(Mood::Narrowing),
            edges(Mood::Asking)
        );
    }

    #[test]
    fn a_running_turn_does_not_look_like_an_idle_one() {
        assert_ne!(
            wound(Mood::Working, 400).dots(COLUMNS),
            wound(Mood::Resting, 400).dots(COLUMNS)
        );
    }

    #[test]
    fn the_tape_does_not_grow_without_end() {
        // It runs for as long as the session does. Only what is on screen is worth keeping.
        let trace = wound(Mood::Working, 10_000);
        assert!(
            trace.written.len() <= COLUMNS * 2,
            "it kept {} samples",
            trace.written.len()
        );
    }

    #[test]
    fn any_width_draws_without_panicking() {
        // A display narrower than a signal, and one wider than the tape has filled.
        for width in 2..40 {
            let mut trace = Trace::default();
            for tick in 0..30 {
                trace.advance(Mood::Working, tick, width);
            }
            assert_eq!(trace.dots(width).len(), width, "at width {width}");
        }
    }
}
