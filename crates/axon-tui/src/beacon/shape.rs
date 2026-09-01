//! What each state puts on the wire, and the tape it is written onto.
//!
//! The display is one instrument — a monitor — and the states differ in the signal running
//! through it, not in what kind of thing they are. The trace scrolls, the way an ECG does: new
//! samples arrive at the right and the line runs off to the left, continuously, always the full
//! width. A flat line scrolling is a flat line, and that is exactly what it should be when
//! nothing is running.
//!
//! **The tape is what makes a change of state readable.** Each state used to compute its own
//! position from the frame counter, so switching from a heartbeat to a flat line teleported the
//! trace — a new picture appearing where the old one had been, which is a glitch and not a
//! transition. Here there is one tape scrolling at one speed forever, and a state change only
//! changes what is written onto the right-hand end of it. The heartbeat you were watching
//! scrolls off the left in its own time while the flat line comes in behind it.

use super::{Mood, ROWS};

/// Lit dots, by column and then row. Row zero is the top.
pub(super) type Dots = Vec<[bool; ROWS]>;

/// A sample with nothing on the wire at all.
///
/// Not a height, and it lights nothing — the lead is off. Kept in the same table as the heights
/// so the gap scrolls along with everything else instead of being a second mechanism.
const GAP: u8 = u8::MAX;

/// The height everything rests at.
const LINE: u8 = 1;

/// One heartbeat, as a height per sample.
///
/// Zero is the bottom row and three the top. Read left to right it is what a monitor draws: the
/// baseline, the small P bump, the Q dip, the tall R spike, the S dip under the line, the
/// broader T wave, and then a rest longer than everything before it. The rest is what makes it a
/// pulse — a waveform with no pause in it reads as a signal, not a heart.
///
/// Nearly twice as long as the display is wide, and mostly flat, so what you see most of the
/// time is a flat line with a beat travelling through it rather than a wall of waveform.
const HEARTBEAT: [u8; 32] = [
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // the line
    2, 1, // P
    0, 3, 3, 0, // QRS
    1, 2, 2, 1, // T
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // and back to the line
];

/// Nothing at all, at the height the other signals rest at.
const FLAT: [u8; 1] = [LINE];

/// A square wave: something is waiting on an answer and will go on waiting.
///
/// Square rather than round on purpose. Every other signal here is something the machine is
/// doing to itself; this one is a prompt, and a prompt should look manufactured.
const SQUARE: [u8; 8] = [3, 3, 3, 3, 0, 0, 0, 0];

/// The same, tighter: a menu narrowing under what you type rather than a question to answer.
///
/// Half the period of [`SQUARE`], so twice as many cycles fit the display. That works because
/// the trace scrolls rather than stretching to fit: a table is one period and the display shows
/// as many of them as it has room for.
const CHOPPY: [u8; 4] = [3, 3, 0, 0];

/// The line, dropping out: there is no daemon at the other end.
///
/// A dead line would say that too, except that a dead line is also what a hung display looks
/// like. Gaps arriving and scrolling past are the part that says this end is still running.
const LEADOFF: [u8; 10] = [LINE, LINE, LINE, GAP, GAP, LINE, LINE, LINE, GAP, LINE];

/// What this state puts on the wire.
fn signal(mood: Mood) -> &'static [u8] {
    match mood {
        Mood::Working => &HEARTBEAT,
        Mood::Asking => &SQUARE,
        Mood::Narrowing => &CHOPPY,
        Mood::Away => &LEADOFF,
        Mood::Resting | Mood::Holding => &FLAT,
    }
}

/// The tape the trace is written on: what has come past, newest last.
///
/// State, deliberately, and held by the UI beside its other running animations. Everything else
/// on this screen is a pure function of a frame counter, and this cannot be: what is on the left
/// of the display is what the session was doing a second ago, and no amount of arithmetic over
/// the current state recovers that.
#[derive(Debug, Default)]
pub struct Trace {
    /// Samples, oldest first. Never kept longer than the widest display asked for.
    written: Vec<u8>,
    /// How far into the current signal the next sample comes from.
    at: usize,
    /// What was being written last time, to notice a change of state.
    was: Option<Mood>,
    /// The frame the last sample was written on.
    since: usize,
    /// Fractions of a sample owed but not yet written.
    owed: f32,
}

impl Trace {
    /// Write however many samples the clock says have arrived since the last frame.
    ///
    /// Driven from the frame counter rather than a clock of its own, so a UI that redraws
    /// slowly draws a slower trace rather than skipping most of it.
    pub fn advance(&mut self, mood: Mood, tick: usize, columns: usize) {
        // A new signal is written from its own beginning, so a heartbeat starts at the baseline
        // rather than wherever in the beat the old state happened to leave the index.
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
        // Only what is on screen is worth keeping. Held to twice the width rather than exactly
        // it, so the trim is occasional instead of once a sample.
        if self.written.len() > columns * 2 {
            self.written.drain(..self.written.len() - columns);
        }
    }

    /// The trace as it stands, `columns` wide.
    ///
    /// Joined vertically between neighbouring samples, for the same reason an oscilloscope joins
    /// its own: unconnected, the R spike is a dot floating three rows above a line, which reads
    /// as a speck of dust rather than a beat. A gap joins to nothing on either side.
    pub(super) fn dots(&self, columns: usize) -> Dots {
        let mut dots = vec![[false; ROWS]; columns];
        // Short of a full display, the rest is baseline: a session that has just opened shows a
        // flat line, not a half-drawn one.
        let short = columns.saturating_sub(self.written.len());
        let at = |x: usize| {
            if x < short {
                return LINE;
            }
            // Counted back from the newest sample, so the right-hand edge is always what has
            // just arrived however much of the tape has been written.
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

/// How many samples arrive per frame, so the trace crosses the display in `axon.ui.beacon_ms`.
///
/// One rate for every state. It used to be one per state, which is what made a change of state
/// jump: two different scroll positions computed from the same frame counter are two different
/// pictures, and swapping between them is a cut, not a transition.
fn rate(columns: usize) -> f32 {
    let across = crate::metric::beacon_ms().max(1) as f32;
    columns as f32 * crate::metric::frame_ms().max(1) as f32 / across
}

/// One tape, one speed, and a different signal written onto it for each state.
#[cfg(test)]
mod tests {
    use super::*;

    /// A width to draw at, in dot columns: nine cells, like the built-in.
    const COLUMNS: usize = 18;

    /// A trace wound forward `frames` frames with `mood` on the wire.
    fn wound(mood: Mood, frames: usize) -> Trace {
        let mut trace = Trace::default();
        for tick in 0..frames {
            trace.advance(mood, tick, COLUMNS);
        }
        trace
    }

    /// The heights of a drawn trace, top row first per column.
    fn rows(dots: &Dots) -> Vec<Vec<usize>> {
        dots.iter()
            .map(|column| (0..ROWS).filter(|row| column[*row]).collect())
            .collect()
    }

    #[test]
    fn a_fresh_trace_is_a_flat_line() {
        // Nothing has come past yet, and a half-drawn display would read as a UI still starting.
        let dots = Trace::default().dots(COLUMNS);
        for (x, on) in rows(&dots).iter().enumerate() {
            assert_eq!(on, &vec![ROWS - 2], "column {x} is not on the line");
        }
    }

    #[test]
    fn the_trace_scrolls() {
        // Whatever else it does, it has to be visibly moving while a turn is running.
        let mut trace = wound(Mood::Working, 40);
        let before = trace.dots(COLUMNS);
        for tick in 40..64 {
            trace.advance(Mood::Working, tick, COLUMNS);
        }
        assert_ne!(before, trace.dots(COLUMNS), "it never moved");
    }

    #[test]
    fn a_change_of_state_writes_nothing_on_its_own() {
        // The whole reason there is a tape. Switching signals within one frame must not move
        // the trace: what is on screen already happened, and it does not get to change
        // retroactively because the agent finished.
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
        // And with time passing, the display that follows is the one before it shifted along --
        // not a different picture in the same place. Each state used to work out its own
        // position from the frame counter, so a turn ending teleported the trace.
        let mut trace = wound(Mood::Working, 200);
        let before = trace.dots(COLUMNS);
        trace.advance(Mood::Resting, 200, COLUMNS);
        let after = trace.dots(COLUMNS);
        // From the second column in. The leftmost one lost the neighbour it was joined to when
        // that neighbour scrolled off, so it is legitimately drawn differently from how it was
        // drawn a frame ago -- it is the join that changed, not the sample.
        let kept = COLUMNS - 6;
        let shifted = (1..=4).any(|by| before[by + 1..by + 1 + kept] == after[1..1 + kept]);
        assert!(
            shifted,
            "the display is not the one before it moved along:\n{before:?}\n{after:?}"
        );
    }

    #[test]
    fn a_new_signal_arrives_from_the_right() {
        // And having not jumped, it has to actually change -- by scrolling in, one sample at a
        // time, so a beat you were watching finishes its run off the left.
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
        // Far enough in to have written something, and not so far that the beat has come round.
        // Measured off `at`, which counts what this signal has written, rather than off an index
        // into the tape -- the tape is trimmed as it grows and any index into it goes stale.
        // One advance first, because until the new signal has been written once `at` is still
        // the old one's count and a loop guarded on it never runs.
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
        // The R wave climbs three rows in one sample. Unjoined that is a dot above a gap above a
        // dot, which is a speck of dust rather than a beat.
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
        // Both are waiting on you and not for the same thing: one wants an answer, the other
        // narrows under what you type. Same family, different beat -- counted as edges once
        // drawn, because a table is a period and the width is what sets the frequency.
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
        // The distinction the whole display exists for.
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
