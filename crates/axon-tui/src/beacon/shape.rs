//! What each state puts on the wire, and the beam that draws it.
//!
//! The display is one instrument — a monitor — and the states differ in the signal running
//! through it, not in what kind of thing they are. A beam sweeps left to right drawing the trace
//! and wiping what was there, the way an ECG does, so every signal moves even when the signal
//! itself is flat. That is the whole reason a flat line works here at all: on its own it is a
//! static picture, and with a beam crossing it, it is an instrument saying nothing is happening.
//!
//! Split from the module that packs and colours the dots because they are two different
//! questions. Here is only geometry over time: a phase from zero to one goes in and a grid of
//! lit dots comes out, with no idea what a braille cell is.

use super::{Mood, ROWS};

/// Lit dots, by column and then row. Row zero is the top.
pub(super) type Dots = Vec<[bool; ROWS]>;

/// One heartbeat, as a height per sample.
///
/// Zero is the bottom row and three the top. Read left to right it is what a monitor draws: the
/// baseline, the small P bump, the Q dip, the tall R spike, the S dip under the line, the
/// broader T wave, and then a rest longer than everything before it. The rest is what makes it a
/// pulse — a waveform with no pause in it reads as a signal, not a heart.
const HEARTBEAT: [u8; 18] = [1, 1, 1, 2, 1, 0, 3, 3, 0, 1, 1, 2, 2, 1, 1, 1, 1, 1];

/// Nothing at all, at the height the other signals rest at.
const FLAT: [u8; 1] = [1];

/// A square wave: something is waiting on an answer and will go on waiting.
///
/// Square rather than round on purpose. Every other signal here is something the machine is
/// doing to itself; this one is a prompt, and a prompt should look manufactured.
const SQUARE: [u8; 8] = [3, 3, 3, 3, 0, 0, 0, 0];

/// The same, tighter: a menu narrowing under what you type rather than a question to answer.
///
/// Three cycles where [`SQUARE`] has one, and that is the whole difference -- so it has to be
/// written as three. A four-sample table stretched over the display draws exactly the same one
/// cycle the eight-sample one does, which is how these two came out pixel-identical the first
/// time: the table is a shape, not a frequency, and the width is what sets the frequency.
const CHOPPY: [u8; 12] = [3, 3, 0, 0, 3, 3, 0, 0, 3, 3, 0, 0];

/// What this state puts on the wire.
fn signal(mood: Mood) -> &'static [u8] {
    match mood {
        Mood::Working => &HEARTBEAT,
        Mood::Asking => &SQUARE,
        Mood::Narrowing => &CHOPPY,
        Mood::Resting | Mood::Holding | Mood::Away => &FLAT,
    }
}

/// What this state draws at this point in its cycle.
pub(super) fn draw(mood: Mood, phase: f32, columns: usize) -> Dots {
    if mood == Mood::Away {
        return breaking(phase, columns);
    }
    monitor(signal(mood), phase, columns)
}

/// A beam sweeping left to right, drawing `signal` behind it and leaving blank ahead of it.
///
/// The trace is joined vertically between neighbouring samples, for the same reason an
/// oscilloscope joins its own: unconnected, the R spike is a dot floating three rows above a
/// line, which reads as a speck of dust rather than a beat.
fn monitor(signal: &[u8], phase: f32, columns: usize) -> Dots {
    let mut dots = vec![[false; ROWS]; columns];
    let beam = (phase * columns as f32) as usize;
    let at = |x: usize| {
        let index = x * signal.len() / columns.max(1);
        usize::from(signal[index.min(signal.len() - 1)])
    };
    let drawn = beam.min(columns.saturating_sub(1));
    for (x, column) in dots.iter_mut().enumerate().take(drawn + 1) {
        let here = at(x);
        let before = if x == 0 { here } else { at(x - 1) };
        for height in here.min(before)..=here.max(before) {
            column[ROWS - 1 - height.min(ROWS - 1)] = true;
        }
    }
    dots
}

/// A flat line with a gap travelling through it: the lead is off.
///
/// Not the monitor, because there is nothing to sweep — the other end is gone. A dead line would
/// say that too, except that a dead line is also what a hung display looks like. The gap moving
/// is the part that says this end is still running.
fn breaking(phase: f32, columns: usize) -> Dots {
    let mut dots = vec![[false; ROWS]; columns];
    let gap = (phase * columns as f32) as usize % columns.max(1);
    for (x, column) in dots.iter_mut().enumerate() {
        if x != gap && x != (gap + 1) % columns {
            column[ROWS - 2] = true;
        }
    }
    dots
}

/// One instrument, one beam, and a different signal on the wire for each state.
#[cfg(test)]
mod tests {
    use super::*;

    /// Where in a cycle to sample. Enough to catch a signal only wrong at one end.
    const STEPS: usize = 64;

    /// A width to draw at, in dot columns: nine cells, like the built-in.
    const COLUMNS: usize = 18;

    const EVERY: [Mood; 6] = [
        Mood::Resting,
        Mood::Holding,
        Mood::Working,
        Mood::Narrowing,
        Mood::Asking,
        Mood::Away,
    ];

    /// How many dots are lit, over the whole display.
    fn lit(dots: &Dots) -> usize {
        dots.iter().flatten().filter(|on| **on).count()
    }

    #[test]
    fn every_state_moves() {
        // The reason there is a beam at all. A flat line is a static picture on its own, and a
        // static picture in the footer is indistinguishable from a UI that has stopped.
        for mood in EVERY {
            let first = draw(mood, 0.0, COLUMNS);
            assert!(
                (1..STEPS).any(|step| draw(mood, step as f32 / STEPS as f32, COLUMNS) != first),
                "{mood:?} never changes"
            );
        }
    }

    #[test]
    fn the_beam_fills_the_display_and_starts_over() {
        // Wipe and redraw, which is what gives a signal that never varies something to do. Not
        // `Away`: there is no beam when the other end is gone, only a gap travelling along a
        // line that is always the same length.
        for mood in EVERY.into_iter().filter(|m| *m != Mood::Away) {
            let drawn: Vec<usize> = (0..STEPS)
                .map(|step| lit(&draw(mood, step as f32 / STEPS as f32, COLUMNS)))
                .collect();
            let most = drawn.iter().copied().max().unwrap_or(0);
            let least = drawn.iter().copied().min().unwrap_or(0);
            assert!(most > least, "{mood:?} draws the same amount throughout");
        }
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
        let resting = HEARTBEAT.iter().filter(|h| **h == 1).count();
        assert!(
            resting * 2 > HEARTBEAT.len(),
            "and it spends most of the beat at rest"
        );
    }

    #[test]
    fn the_heartbeat_is_drawn_as_a_joined_trace() {
        // The R wave climbs three rows in one sample. Unjoined that is a dot above a gap above a
        // dot, which is a speck of dust rather than a beat.
        let dots = monitor(&HEARTBEAT, 1.0, COLUMNS);
        for (x, column) in dots.iter().enumerate() {
            let on: Vec<usize> = (0..ROWS).filter(|row| column[*row]).collect();
            assert!(!on.is_empty(), "column {x} is empty");
            assert_eq!(
                on.last().expect("lit") - on[0] + 1,
                on.len(),
                "column {x} has a hole in it: {on:?}"
            );
        }
    }

    #[test]
    fn a_flat_line_is_flat() {
        // Nothing is happening, and the display should be ignorable while that is true.
        let dots = monitor(&FLAT, 1.0, COLUMNS);
        for (x, column) in dots.iter().enumerate() {
            let on: Vec<usize> = (0..ROWS).filter(|row| column[*row]).collect();
            assert_eq!(on, vec![ROWS - 2], "column {x} is off the line");
        }
    }

    #[test]
    fn the_square_wave_only_has_two_heights() {
        // Manufactured-looking on purpose: it is the one signal that is a prompt rather than
        // something the machine is doing to itself.
        for wave in [SQUARE.as_slice(), CHOPPY.as_slice()] {
            let mut seen: Vec<u8> = wave.to_vec();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), 2, "{wave:?} is not a square wave");
        }
    }

    #[test]
    fn a_menu_and_a_permission_are_not_the_same_square() {
        // Both are waiting on you and they are not waiting for the same thing: one wants an
        // answer, the other narrows under what you type. Same family, different beat.
        //
        // Counted as edges *once drawn*, not as table lengths. Two tables of different lengths
        // holding the same one cycle stretch to the same picture, which is exactly what these
        // two did to begin with.
        let edges = |mood: Mood| {
            let dots = draw(mood, 1.0, COLUMNS);
            (1..COLUMNS).filter(|x| dots[*x] != dots[x - 1]).count()
        };
        assert!(
            edges(Mood::Narrowing) > edges(Mood::Asking),
            "a narrowing menu is not busier than a permission ask: {} against {}",
            edges(Mood::Narrowing),
            edges(Mood::Asking)
        );
        assert!(
            (0..STEPS).any(|step| {
                let at = step as f32 / STEPS as f32;
                draw(Mood::Asking, at, COLUMNS) != draw(Mood::Narrowing, at, COLUMNS)
            }),
            "they draw the same thing throughout"
        );
    }

    #[test]
    fn thinking_does_not_look_like_not_thinking() {
        // The one distinction the whole display exists for.
        assert!(
            (0..STEPS).any(|step| {
                let at = step as f32 / STEPS as f32;
                draw(Mood::Working, at, COLUMNS) != draw(Mood::Resting, at, COLUMNS)
            }),
            "a running turn draws the same as an idle session"
        );
    }

    #[test]
    fn the_lead_off_line_always_has_a_gap_in_it() {
        // The gap moving is what says this end is still running. A line with no gap is a line.
        for step in 0..STEPS {
            let dots = breaking(step as f32 / STEPS as f32, COLUMNS);
            assert!(
                dots.iter().any(|column| column.iter().all(|on| !on)),
                "step {step} has no gap"
            );
        }
    }

    #[test]
    fn a_signal_shorter_than_the_display_still_fills_it() {
        // `FLAT` is one sample and `CHOPPY` is four, on eighteen columns. Reading past the end
        // of the table would panic, which on a display nobody looks at directly is the worst
        // possible place for it.
        for mood in EVERY {
            for width in 2..40 {
                let dots = draw(mood, 0.99, width);
                assert_eq!(dots.len(), width, "{mood:?} at width {width}");
            }
        }
    }
}
