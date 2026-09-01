//! What each state puts on the wire.
//!
//! The display is one instrument — a monitor — and the states differ in the signal running
//! through it, not in what kind of thing they are. The trace scrolls, the way an ECG does: new
//! samples arrive at the right and the line runs off to the left, continuously, always the full
//! width. A flat line scrolling is a flat line, and that is exactly what it should be when
//! nothing is running.
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
const FLAT: [u8; 1] = [1];

/// A square wave: something is waiting on an answer and will go on waiting.
///
/// Square rather than round on purpose. Every other signal here is something the machine is
/// doing to itself; this one is a prompt, and a prompt should look manufactured.
const SQUARE: [u8; 8] = [3, 3, 3, 3, 0, 0, 0, 0];

/// The same, tighter: a menu narrowing under what you type rather than a question to answer.
///
/// Half the period of [`SQUARE`], so twice as many cycles fit the display. That works because
/// the trace scrolls rather than stretching to fit: a table is one period and the display shows
/// as many of them as it has room for. While it did stretch, these two drew pixel-for-pixel the
/// same picture — both were one cycle wide however many samples they were written with.
const CHOPPY: [u8; 4] = [3, 3, 0, 0];

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

/// The trace, scrolling: new samples arrive at the right and the line runs off to the left.
///
/// Continuous, and always the full width of the display. It swept before -- a beam crossing and
/// wiping -- which meant half the display was blank at any moment and the trace was something
/// being erased rather than something arriving. A monitor scrolls; the line is always there and
/// what changes is what has just come in.
///
/// The trace is joined vertically between neighbouring samples, for the same reason an
/// oscilloscope joins its own: unconnected, the R spike is a dot floating three rows above a
/// line, which reads as a speck of dust rather than a beat.
fn monitor(signal: &[u8], phase: f32, columns: usize) -> Dots {
    let mut dots = vec![[false; ROWS]; columns];
    let scrolled = (phase * signal.len() as f32) as usize;
    let at = |x: usize| usize::from(signal[(scrolled + x) % signal.len()]);
    for (x, column) in dots.iter_mut().enumerate() {
        let here = at(x);
        // Joined to the sample on its left, so a climb is a stroke and not two dots with air
        // between them. The leftmost has nothing to its left and stands alone.
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

/// One instrument, one scrolling trace, and a different signal on the wire for each state.
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

    #[test]
    fn every_state_with_something_to_say_moves() {
        // Resting and holding are not in this list, and that is the point of them: a flat line
        // scrolling is a flat line, and a display that holds still is the clearest way to say
        // nothing is running. Everything else has to be visibly doing something.
        for mood in EVERY
            .into_iter()
            .filter(|m| !matches!(m, Mood::Resting | Mood::Holding))
        {
            let first = draw(mood, 0.0, COLUMNS);
            assert!(
                (1..STEPS).any(|step| draw(mood, step as f32 / STEPS as f32, COLUMNS) != first),
                "{mood:?} never changes"
            );
        }
    }

    #[test]
    fn the_trace_is_always_the_full_width() {
        // It swept before, wiping and redrawing, which left half the display blank at any
        // moment. A monitor scrolls: the line is always there and what changes is what has just
        // come in. `Away` is the exception it is meant to be -- that gap is the whole message.
        for mood in EVERY.into_iter().filter(|m| *m != Mood::Away) {
            for step in 0..STEPS {
                let dots = draw(mood, step as f32 / STEPS as f32, COLUMNS);
                let empty = dots.iter().filter(|c| c.iter().all(|on| !on)).count();
                assert_eq!(
                    empty, 0,
                    "{mood:?} at step {step} has {empty} blank columns"
                );
            }
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
        // Counted as edges *once drawn*, not as table lengths. While the trace stretched to fit,
        // two tables of different lengths holding the same one cycle drew the same picture --
        // which is exactly what these two did to begin with.
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
