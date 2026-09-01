//! What each state draws: which dots are lit, and how hot each column is.
//!
//! Split from the module that colours and packs them because they are two different questions.
//! Here is only geometry over time — a phase from zero to one goes in and a [`Shape`] comes
//! out, with no idea what a braille cell or a palette is.

use super::{Mood, ROWS};

/// What one frame of a state looks like: which dots are lit, and how hot each column is.
///
/// The two are separate because they answer different questions. A column can be lit and cold —
/// the fringe of a scanner, the far edge of a breath — and drawing that with one colour for the
/// whole strip is what made the first version of this read as a progress bar.
pub(super) struct Shape {
    /// Lit dots, by column and then row. Row zero is the top.
    pub(super) dots: Vec<[bool; ROWS]>,
    /// How much is going on in each column, from 0.0 to 1.0.
    pub(super) heat: Vec<f32>,
}

impl Shape {
    /// An empty display of the configured width.
    fn blank(columns: usize) -> Self {
        Self {
            dots: vec![[false; ROWS]; columns],
            heat: vec![0.0; columns],
        }
    }
}

/// A phase turned into a there-and-back journey that eases into both ends.
///
/// Zero at the start, one at the halfway point, zero again at the end, and slow at all three.
/// This is the whole reason the scanner reads as a machine rather than a rectangle: a linear
/// sweep arrives at the wall at full speed and reverses in one frame, which nothing physical
/// does. A cosine costs the same and decelerates into every turn for free.
fn swing(phase: f32) -> f32 {
    (phase * std::f32::consts::TAU).cos().mul_add(-0.5, 0.5)
}

/// What this state draws at this point in its cycle.
///
/// Split from [`render`] so a test can say what a state draws without going through spans and
/// colours to find out.
pub(super) fn draw(mood: Mood, phase: f32, columns: usize) -> Shape {
    match mood {
        Mood::Resting => wave(phase, 0.8, 1.2, columns),
        Mood::Holding => wave(phase, 1.6, 1.5, columns),
        Mood::Working => scanner(phase, columns),
        Mood::Narrowing => narrowing(phase, columns),
        Mood::Asking => breath(phase, columns),
        Mood::Away => dropout(phase, columns),
    }
}

/// A sine drifting right, `height` rows tall and `waves` of it across the display.
///
/// Drawn as a joined line rather than a dot per column: the same reason an oscilloscope draws
/// one. Unjoined, a wave with any real amplitude is a scatter of dots that reads as noise. The
/// heat is how high the line has climbed, so the crests are the lit part and the troughs sit
/// back in the border grey — which is what gives a wave only two rows tall anything to read.
fn wave(phase: f32, height: f32, waves: f32, columns: usize) -> Shape {
    let at = |x: usize| {
        let turn = phase + x as f32 / columns as f32 * waves;
        (turn * std::f32::consts::TAU).sin()
    };
    let middle = (ROWS as f32 - 1.0) / 2.0;
    let mut shape = trace(|x| middle + at(x) * height / 2.0, columns);
    for x in 0..columns {
        shape.heat[x] = at(x).mul_add(0.5, 0.5);
    }
    shape
}

/// How much of the display the lamp covers, core and fringe together.
///
/// A share rather than a count. Fixed at five columns it was most of a narrow display and a dot
/// on a wide one; three fifths looks like the same lamp at every width somebody might configure.
const SPREAD: f32 = 0.6;

/// How many dot columns either side of the lamp's core are lit, on a display this wide.
fn reach(columns: usize) -> f32 {
    (columns as f32 * SPREAD / 2.0).max(1.5)
}

/// The rows the scanner uses: the middle two, and only ever those.
///
/// It filled all four before, which on a nine-cell display is a blob the size of the footer's
/// whole middle. Two rows is a lamp on a track; four is a bar chart having a moment.
const TRACK: [usize; 2] = [1, 2];

/// What fraction of one crossing the scanner spends waiting out of sight at the end of it.
const DWELL: f32 = 0.24;

/// How long each crossing takes, relative to the others.
///
/// Four of them, and no two the same, so the display never settles into a beat you can predict.
/// A lamp that crosses in exactly the time it took last time is a metronome, and a metronome in
/// the corner of the eye is the kind of thing you start waiting for instead of working through.
/// Uneven, it stays something glanced at.
///
/// They average to one, so `axon.ui.beacon_ms` still means what it says: change the numbers to
/// change the character of it, and the pace setting to change the speed.
const PASSES: [f32; 4] = [1.35, 0.8, 1.55, 0.9];

/// A lamp running a track: three fifths of the display wide, brightest in the middle, and off
/// the end at each turn.
///
/// The one thing on this display that was replaced rather than kept. It was a comet -- a head
/// with its tail on one side -- which is a shape that looks right going one way and wrong going
/// the other. This is symmetric about its own core, so it looks the same in both directions and
/// only the direction changes.
///
/// It leaves the screen completely at each end and waits there before coming back. A scanner
/// that stops with a sliver of itself still showing has not gone anywhere, it has just run out
/// of room; going right off and pausing is the difference between a lamp on a track and a bar
/// that fills up.
fn scanner(phase: f32, columns: usize) -> Shape {
    let mut shape = Shape::blank(columns);
    let Some(core) = running(phase, columns) else {
        return shape;
    };
    for x in 0..columns {
        // Linear falloff from the core, the same in both directions, so the middle column is
        // the bright one and the pair either side of it fade out.
        let lit = (1.0 - (x as f32 - core).abs() / reach(columns)).max(0.0);
        if lit <= 0.0 {
            continue;
        }
        for row in TRACK {
            shape.dots[x][row] = true;
        }
        shape.heat[x] = lit;
    }
    shape
}

/// Where the scanner's core is, or `None` while it is waiting off the end.
///
/// Three things are happening here and they are all timing. The cycle is four crossings rather
/// than one, and [`PASSES`] gives each a different length, so one is quick and the next is
/// unhurried. Within a crossing the travel is [`smooth`]ed rather than linear, because a sweep
/// that arrives at the wall at full speed and reverses in a single frame is a rectangle going
/// back and forth. And each crossing ends with the lamp parked out of sight for [`DWELL`] of it.
///
/// The two ends are a full fringe past the edge, so the last of it is gone before it stops.
fn running(phase: f32, columns: usize) -> Option<f32> {
    let whole: f32 = PASSES.iter().sum();
    let mut at = phase.clamp(0.0, 1.0) * whole;
    let from = -reach(columns);
    let to = columns as f32 + reach(columns);
    for (index, pass) in PASSES.iter().enumerate() {
        if at >= *pass {
            at -= pass;
            continue;
        }
        let moving = 1.0 - DWELL;
        let along = at / pass;
        if along >= moving {
            return None;
        }
        let eased = smooth(along / moving);
        // Every other crossing goes the other way, which is what makes four of them two round
        // trips rather than four runs that all start from the left.
        return Some(if index % 2 == 0 {
            from + eased * (to - from)
        } else {
            to + eased * (from - to)
        });
    }
    None
}

/// Ease in and out of a journey: still at both ends, quickest through the middle.
fn smooth(at: f32) -> f32 {
    at * at * at.mul_add(-2.0, 3.0)
}

/// Two markers coming in from the ends towards the middle, over and over.
///
/// A completion popup is open and every keystroke narrows it, so the display narrows too. It is
/// the one state that had none: `/` opened a menu and the footer went on drawing the slow wave
/// it draws when nothing at all is happening, which is exactly wrong -- something *is*
/// happening, you are in the middle of it, and it is not the same something as a permission ask.
fn narrowing(phase: f32, columns: usize) -> Shape {
    let mut shape = Shape::blank(columns);
    let middle = (columns as f32 - 1.0) / 2.0;
    let closing = smooth(phase) * middle;
    for x in 0..columns {
        let out = (x as f32 - middle).abs();
        let lit = (1.0 - (out - (middle - closing)).abs() / 2.0).max(0.0);
        if lit <= 0.0 {
            continue;
        }
        for row in TRACK {
            shape.dots[x][row] = true;
        }
        shape.heat[x] = shape.heat[x].max(lit);
    }
    shape
}
/// A bar in the middle breathing out to the edges and back.
///
/// Symmetric on purpose. Everything else here travels, and a thing that grows in place is the
/// one shape that does not read as progress — which is the point: nothing is progressing, it is
/// your move. The heat is the breath itself rather than the distance from the middle, so the
/// whole display brightens as it opens instead of the edges always being the cold part.
fn breath(phase: f32, columns: usize) -> Shape {
    let mut shape = Shape::blank(columns);
    let open = swing(phase);
    // Never all the way shut. A frame with nothing lit reads as the UI having died, which is
    // the one thing none of these states mean -- so it bottoms out at the two middle columns.
    let reach = (open * columns as f32 / 2.0).max(0.5);
    let middle = (columns as f32 - 1.0) / 2.0;
    for x in 0..columns {
        let out = (x as f32 - middle).abs();
        if out > reach {
            continue;
        }
        let tall = ROWS.saturating_sub(out as usize).max(1);
        for row in 0..tall {
            shape.dots[x][ROWS - 1 - row] = true;
        }
        shape.heat[x] = open;
    }
    shape
}

/// A flat line with a gap travelling through it.
///
/// A dead line would do as well for "there is no daemon", except that a dead line is also what a
/// hung display looks like. The gap moving is the part that says the UI is still running and it
/// is the other end that is missing. The columns either side of the break are the hot ones, so
/// it reads as an arc across the gap rather than as a line with a bite out of it.
fn dropout(phase: f32, columns: usize) -> Shape {
    let mut shape = Shape::blank(columns);
    let gap = (phase * columns as f32) as usize % columns;
    let after = (gap + 1) % columns;
    for x in 0..columns {
        if x == gap || x == after {
            continue;
        }
        shape.dots[x][ROWS - 2] = true;
        let beside = x == (gap + columns - 1) % columns || x == (after + 1) % columns;
        shape.heat[x] = if beside { 1.0 } else { 0.0 };
    }
    shape
}

/// Light a joined line through the height `at` gives for each column.
fn trace(at: impl Fn(usize) -> f32, columns: usize) -> Shape {
    let mut shape = Shape::blank(columns);
    let row_of = |height: f32| {
        let clamped = height.round().clamp(0.0, ROWS as f32 - 1.0);
        ROWS - 1 - clamped as usize
    };
    for x in 0..columns {
        let here = row_of(at(x));
        let previous = row_of(at(if x == 0 { columns - 1 } else { x - 1 }));
        for row in here.min(previous)..=here.max(previous) {
            shape.dots[x][row] = true;
        }
    }
    shape
}

/// Each shape is what it claims: the right size, in the right place, moving the right way.
#[cfg(test)]
mod tests {
    use super::*;

    /// Where in a cycle to sample. Enough to catch a shape that is only wrong at one end.
    const STEPS: usize = 64;

    /// A width to draw at, in dot columns: nine cells, like the built-in.
    const COLUMNS: usize = 18;

    /// The phase at which the lamp's core is nearest the middle of the display.
    ///
    /// Searched rather than written down: the crossings are deliberately uneven lengths, so
    /// "halfway through the first one" is an arithmetic exercise that goes stale the moment
    /// anybody touches `PASSES`. Asking where it actually is cannot.
    fn centred() -> f32 {
        let middle = (COLUMNS as f32 - 1.0) / 2.0;
        (0..2000)
            .map(|step| step as f32 / 2000.0)
            .filter(|at| running(*at, COLUMNS).is_some())
            .min_by(|a, b| {
                let away = |at: &f32| (running(*at, COLUMNS).unwrap_or(f32::MAX) - middle).abs();
                away(a).total_cmp(&away(b))
            })
            .expect("it crosses the middle at some point")
    }

    #[test]
    fn the_scanner_is_symmetric_about_its_core() {
        // The one thing that changed, and the reason it changed. The comet before it had its
        // tail on one side, which looks right going one way and wrong going the other. Sampled
        // where the core is on the display's own middle, so the two symmetries are one.
        let at = centred();
        let shape = scanner(at, COLUMNS);
        let last = COLUMNS - 1;
        for x in 0..COLUMNS / 2 {
            assert_eq!(
                shape.dots[x],
                shape.dots[last - x],
                "at {at} the core is centred but column {x} is not mirrored"
            );
        }
    }

    #[test]
    fn the_scanner_leaves_the_screen_completely() {
        // A scanner that stops with a sliver still showing has not gone anywhere, it has just
        // run out of room. Off, entirely, at both ends -- and then it waits there.
        let dark: Vec<f32> = (0..STEPS)
            .map(|step| step as f32 / STEPS as f32)
            .filter(|at| scanner(*at, COLUMNS).heat.iter().all(|hot| *hot <= 0.0))
            .collect();
        assert!(!dark.is_empty(), "it is never off the screen");
        assert!(
            dark.iter().any(|at| *at < 0.5) && dark.iter().any(|at| *at > 0.5),
            "it only leaves at one end: {dark:?}"
        );
    }

    #[test]
    fn the_scanner_is_two_rows_and_three_fifths_wide() {
        // Not a blob. Four rows across a nine-cell display is the size of the footer's whole
        // middle. And a share of the width rather than a count of columns, so it is the same
        // lamp on a five-cell display as on a fifteen-cell one.
        let shape = scanner(centred(), COLUMNS);
        let lit: Vec<usize> = (0..COLUMNS).filter(|x| shape.heat[*x] > 0.0).collect();
        let want = (COLUMNS as f32 * SPREAD).round() as usize;
        // Give or take a column, for a core sitting between two rather than on one -- which is
        // the fringe being drawn honestly, not the shape growing.
        assert!(
            lit.len().abs_diff(want) <= 1,
            "wanted about {want} columns, got {}: {lit:?}",
            lit.len()
        );
        for x in lit {
            let rows: Vec<usize> = (0..ROWS).filter(|row| shape.dots[x][*row]).collect();
            assert_eq!(rows, vec![1, 2], "column {x} is not the middle two rows");
        }
    }

    #[test]
    fn the_scanner_is_brightest_in_its_middle() {
        let shape = scanner(centred(), COLUMNS);
        let hottest = shape
            .heat
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(x, _)| x)
            .expect("a lit column");
        for x in 0..COLUMNS {
            if x.abs_diff(hottest) < 2 {
                continue;
            }
            let inner = if x > hottest { x - 1 } else { x + 1 };
            assert!(
                shape.heat[x] <= shape.heat[inner] + f32::EPSILON,
                "column {x} is hotter than one closer to the core: {:?}",
                shape.heat
            );
        }
    }

    #[test]
    fn the_scanner_slows_into_its_turns() {
        // A linear sweep hits the wall at full speed and reverses in one frame, which nothing
        // physical does. Measured as distance covered over one first crossing: least where it
        // sets off, most as it goes through the middle.
        let whole: f32 = PASSES.iter().sum();
        let first = PASSES[0] / whole;
        let step = 0.002;
        let moved = |along: f32| {
            let at = |p: f32| running(p * first, COLUMNS).expect("still travelling");
            (at(along + step) - at(along)).abs()
        };
        assert!(
            moved(0.38) > moved(0.01) * 3.0,
            "the middle of a crossing is not much faster than its start"
        );
        assert!(
            moved(0.38) > moved(0.72) * 3.0,
            "it does not slow into the far end either"
        );
    }

    #[test]
    fn no_two_crossings_take_the_same_time() {
        // A lamp that crosses in exactly the time it took last time is a metronome, and a
        // metronome in the corner of the eye is a thing you wait for instead of work through.
        for (index, pass) in PASSES.iter().enumerate() {
            for other in &PASSES[index + 1..] {
                assert!(
                    (pass - other).abs() > 0.05,
                    "two crossings of {pass} and {other} are the same beat"
                );
            }
        }
    }

    #[test]
    fn a_crossing_is_quicker_than_a_resting_wave() {
        // Working's whole cycle is much longer than anything else here, because it is four
        // crossings and two pauses. That is not the same as it being slow, and this is the
        // measurement that says so: one crossing, against one turn of the resting wave.
        let whole: f32 = PASSES.iter().sum();
        let cycle = |mood: Mood| {
            let (slower, faster) = mood.pace();
            crate::metric::beacon_ms() as f32 * slower as f32 / faster as f32
        };
        let longest = PASSES.iter().copied().fold(0.0, f32::max);
        let crossing = cycle(Mood::Working) * (longest / whole) * (1.0 - DWELL);
        assert!(
            crossing < cycle(Mood::Resting),
            "a crossing takes {crossing}ms against a resting wave's {}ms",
            cycle(Mood::Resting)
        );
    }
    #[test]
    fn narrowing_closes_on_the_middle() {
        // A completion popup narrows as you type, so this does too. Two markers, coming in.
        let out = |at: f32| {
            let shape = narrowing(at, COLUMNS);
            let middle = (COLUMNS as f32 - 1.0) / 2.0;
            (0..COLUMNS)
                .filter(|x| shape.heat[*x] > 0.0)
                .map(|x| (x as f32 - middle).abs())
                .fold(0.0, f32::max)
        };
        assert!(
            out(0.9) < out(0.1),
            "it is no closer to the middle at the end than the start"
        );
    }

    #[test]
    fn the_breath_is_symmetric() {
        // It grows in place rather than travelling, which is the one shape here that does not
        // read as progress -- because nothing is progressing, it is your move.
        for step in 0..STEPS {
            let shape = breath(step as f32 / STEPS as f32, COLUMNS);
            let last = COLUMNS - 1;
            for x in 0..COLUMNS / 2 {
                assert_eq!(
                    shape.dots[x],
                    shape.dots[last - x],
                    "step {step} column {x} is not mirrored"
                );
            }
        }
    }

    #[test]
    fn the_breath_brightens_as_it_opens() {
        // The heat is the breath itself, not the distance from the middle: a display where the
        // edges are always the cold part does not breathe, it just gets wider.
        let hottest = |shape: &Shape| shape.heat.iter().copied().fold(0.0, f32::max);
        assert!(
            hottest(&breath(0.5, COLUMNS)) > hottest(&breath(0.0, COLUMNS)),
            "it is no brighter open than shut"
        );
    }

    #[test]
    fn the_dropout_always_has_a_gap_in_it() {
        // The gap moving is what says the UI is still running and it is the other end that is
        // missing. A line with no gap is just a line.
        for step in 0..STEPS {
            let shape = dropout(step as f32 / STEPS as f32, COLUMNS);
            assert!(
                shape
                    .dots
                    .iter()
                    .any(|column| column.iter().all(|lit| !lit)),
                "step {step} has no gap"
            );
        }
    }

    #[test]
    fn a_wave_is_a_joined_line() {
        // Unjoined, a wave with any real amplitude is a scatter of dots that reads as noise.
        let shape = wave(0.0, 3.0, 1.5, COLUMNS);
        for x in 0..COLUMNS {
            let lit: Vec<usize> = (0..ROWS).filter(|row| shape.dots[x][*row]).collect();
            assert!(!lit.is_empty(), "column {x} is empty");
            assert_eq!(
                lit.last().expect("lit") - lit[0] + 1,
                lit.len(),
                "column {x} has a hole in it: {lit:?}"
            );
        }
    }
}
