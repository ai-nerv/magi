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
//! on purpose: **every animation here is symmetric about the middle**, and a display with an
//! even number of cells has no middle to be symmetric about. The first version of this was a
//! comet with its tail on one side, and a lopsided thing sliding past reads as broken.
//!
//! **Every column carries a heat as well as a shape**, and the cells are coloured from it. The
//! dots say where the energy is and the colour says how much: the core of a scanner is accent
//! and its fringes fall away to the border grey the prompt box is drawn in.
//!
//! **Nothing here moves at a constant rate.** A scanner that crosses at one speed and turns
//! round instantly is a rectangle going back and forth; one that eases into the turn is a
//! machine. The easing is one cosine, and every state that travels uses it.
//!
//! Nothing here is a clock, either. Every animation is a phase from the frame counter and the
//! two settings that bracket it, so a state runs at the rate `axon.ui.beacon_ms` asks for
//! whatever the frame rate happens to be.

use crate::colour;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// Dot rows down the display.
const ROWS: usize = 4;

/// How many cells wide it is, as configured.
#[must_use]
pub fn cells() -> usize {
    usize::from(crate::metric::beacon_cells()).max(1)
}

/// Dot columns across it: two to a cell.
fn columns() -> usize {
    cells() * 2
}

/// What the session is doing, as the display draws it.
///
/// The same states the border's scan knows about, plus the two it does not: the daemon being
/// away, and something on screen waiting for an answer. Both were invisible here before, and
/// both are exactly when a person stares at the footer wondering why nothing is happening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    /// Nothing typed, nothing running. Rings going out from the middle, slowly.
    Resting,
    /// Something is in the prompt and not sent yet. A standing wave, plucked.
    Holding,
    /// A turn is running. A scanner crossing and coming back, easing into each turn.
    Working,
    /// A list or a permission is open and it is your move. A breath, in and out.
    Asking,
    /// The daemon is not there. A flat line breaking open from the middle.
    Away,
}

impl Mood {
    /// How long one cycle of this state's animation takes, as a multiple of the setting.
    ///
    /// Resting is slower than the setting and working is faster, because the two are saying
    /// opposite things and a display that moves at one speed says only that it is on.
    fn pace(self) -> (u64, u64) {
        match self {
            Self::Resting => (2, 1),
            Self::Holding => (1, 1),
            Self::Working => (4, 5),
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
            Self::Asking => [colour::border(), colour::dim(), colour::warning()],
            Self::Away => [colour::border(), colour::dim(), colour::error()],
        }
    }
}

/// What one frame of a state looks like: which dots are lit, and how hot each column is.
///
/// The two are separate because they answer different questions. A column can be lit and cold —
/// the fringe of a scanner, the far edge of a breath — and drawing that with one colour for the
/// whole strip is what made the first version of this read as a progress bar.
struct Shape {
    /// Lit dots, by column and then row. Row zero is the top.
    dots: Vec<[bool; ROWS]>,
    /// How much is going on in each column, from 0.0 to 1.0.
    heat: Vec<f32>,
}

impl Shape {
    /// An empty display of the configured width.
    fn blank() -> Self {
        Self {
            dots: vec![[false; ROWS]; columns()],
            heat: vec![0.0; columns()],
        }
    }

    /// Light `column` to `strength`, as a bar growing from the middle row outwards.
    ///
    /// Vertically centred rather than growing off the floor, because the whole display is built
    /// around a middle and a bar that grows one way is the same lopsidedness in the other axis.
    fn light(&mut self, column: usize, strength: f32) {
        if strength <= 0.0 || column >= self.dots.len() {
            return;
        }
        // Rows in the order they light up: the line first, then out either side of it.
        const ORDER: [usize; ROWS] = [2, 1, 3, 0];
        let tall = (strength * ROWS as f32).ceil().clamp(1.0, ROWS as f32) as usize;
        for row in &ORDER[..tall] {
            self.dots[column][*row] = true;
        }
        self.heat[column] = self.heat[column].max(strength);
    }
}

/// The display as it stands this frame.
#[must_use]
pub fn render(mood: Mood, tick: usize) -> Vec<Span<'static>> {
    let shape = draw(mood, phase(mood, tick));
    let ramp = mood.ramp();
    (0..cells())
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
fn draw(mood: Mood, phase: f32) -> Shape {
    match mood {
        Mood::Resting => ripple(phase),
        Mood::Holding => standing(phase),
        Mood::Working => scanner(phase),
        Mood::Asking => breath(phase),
        Mood::Away => breaking(phase),
    }
}

/// Where the middle of the display is, in dot columns.
fn middle() -> f32 {
    (columns() as f32 - 1.0) / 2.0
}

/// How brightly a column `away` from something reaching `width` should burn.
///
/// Linear, and zero past the edge of it. Falloff is what gives a shape a core and a fringe
/// instead of a hard edge, and it is the same in every direction — which is what keeps all of
/// this symmetric.
fn falloff(away: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    (1.0 - away / width).max(0.0)
}

/// A scanner crossing and coming back, with a fringe either side of its core.
///
/// The one everybody knows. It was a comet with its tail on one side, which is a shape that
/// looks wrong going one way and right going the other; this is symmetric about its own core,
/// so it looks the same in both directions and only the direction changes. Eased by `swing`,
/// so it slows into each wall and comes back out of it.
fn scanner(phase: f32) -> Shape {
    let mut shape = Shape::blank();
    let span = columns() as f32 - 1.0;
    let core = swing(phase) * span;
    let reach = (columns() as f32 / 4.0).max(1.5);
    for x in 0..columns() {
        shape.light(x, falloff((x as f32 - core).abs(), reach));
    }
    shape
}

/// Rings going out from the middle and off both ends.
///
/// Symmetric by construction: a column is as bright as its distance from the middle says, and
/// two columns the same distance out are the same brightness. Slow, and never very bright --
/// this is the state where nothing is happening, and it should be possible to ignore.
fn ripple(phase: f32) -> Shape {
    let mut shape = Shape::blank();
    let middle = middle();
    let front = phase * (middle + 2.0);
    for x in 0..columns() {
        let out = (x as f32 - middle).abs();
        shape.light(x, falloff((out - front).abs(), 2.5) * 0.7);
    }
    shape
}

/// A standing wave, plucked: tallest in the middle, pinned at both ends, breathing in place.
///
/// Something is typed and not sent. It is not travelling anywhere, because neither are you --
/// what it has instead is amplitude, and the difference between this and the ripple at a glance
/// is that this one stays still and gets louder.
fn standing(phase: f32) -> Shape {
    let mut shape = Shape::blank();
    let span = (columns() as f32 - 1.0).max(1.0);
    let loud = swing(phase).mul_add(0.75, 0.25);
    for x in 0..columns() {
        // A half sine over the width, so it is pinned at both ends and peaks in the middle.
        let along = (x as f32 / span * std::f32::consts::PI).sin();
        shape.light(x, along * loud);
    }
    shape
}

/// A bar in the middle breathing out to the edges and back.
///
/// Everything else here travels or oscillates; this grows in place, which is the one thing that
/// does not read as progress — and that is the point, because nothing is progressing, it is
/// your move. Deliberately unlike the scanner: a person glancing down needs to know whether
/// they are waiting on the machine or it is waiting on them, and that is the whole job of the
/// difference between something crossing and something breathing.
fn breath(phase: f32) -> Shape {
    let mut shape = Shape::blank();
    let open = swing(phase);
    let middle = middle();
    // Never all the way shut. A frame with nothing lit reads as the UI having died, which is
    // the one thing none of these states mean -- so it bottoms out at the two middle columns.
    let reach = (open * (middle + 1.0)).max(0.5);
    for x in 0..columns() {
        let out = (x as f32 - middle).abs();
        if out > reach {
            continue;
        }
        // Tapered towards the edges of the breath rather than flat across it. Flat, a wide
        // breath is a solid block of braille for a third of its cycle, which is a lot of ink
        // for "waiting on you" and stops looking like breathing at all.
        shape.light(x, falloff(out, reach + 1.5) * open.mul_add(0.45, 0.55));
    }
    shape
}

/// A flat line breaking open from the middle, both ways at once.
///
/// A dead line would do as well for "there is no daemon", except that a dead line is also what
/// a hung display looks like. The break moving is what says the UI is still running and it is
/// the other end that is missing, and it opens symmetrically so it reads as the line parting
/// rather than as something travelling along it.
fn breaking(phase: f32) -> Shape {
    let mut shape = Shape::blank();
    let middle = middle();
    // Parts and comes back together rather than opening and snapping shut, and never fully
    // either way: at its narrowest the two middle columns are missing, at its widest the two
    // outermost survive. A frame with no gap is a working line and a frame with nothing lit is
    // a dead UI, and this state is neither.
    let open = swing(phase).mul_add(middle - 1.5, 1.0);
    for x in 0..columns() {
        let out = (x as f32 - middle).abs();
        if out < open {
            continue;
        }
        // Hottest right at the break, which is the part that is moving.
        shape.light(x, falloff(out - open, 2.0).mul_add(0.7, 0.3));
    }
    shape
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

    const EVERY: [Mood; 5] = [
        Mood::Resting,
        Mood::Holding,
        Mood::Working,
        Mood::Asking,
        Mood::Away,
    ];

    /// Where in a cycle to sample. Enough to catch a state that is only wrong at one end.
    const STEPS: usize = 64;

    /// The cells as one string.
    fn strip(mood: Mood, tick: usize) -> String {
        render(mood, tick)
            .iter()
            .map(|s| s.content.to_string())
            .collect()
    }

    /// The colour of each cell.
    fn colours(mood: Mood, tick: usize) -> Vec<Option<Color>> {
        render(mood, tick).iter().map(|s| s.style.fg).collect()
    }

    #[test]
    fn it_is_always_the_configured_width_in_braille() {
        for mood in EVERY {
            for tick in 0..STEPS {
                let out = strip(mood, tick);
                assert_eq!(out.chars().count(), cells(), "{mood:?} at {tick}: {out:?}");
                assert!(
                    out.chars().all(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
                    "{mood:?} at {tick}: {out:?}"
                );
            }
        }
    }

    /// The states that stay put. The scanner travels, so it is symmetric about its own core
    /// instead -- see `the_scanner_is_symmetric_about_its_core`.
    const STATIONARY: [Mood; 4] = [Mood::Resting, Mood::Holding, Mood::Asking, Mood::Away];

    #[test]
    fn every_stationary_state_is_symmetric_about_the_middle() {
        // The reason the cell count is odd. A shape with its weight on one side looks wrong
        // going one way and right going the other, which is what the comet before this did.
        for mood in STATIONARY {
            for step in 0..STEPS {
                let shape = draw(mood, step as f32 / STEPS as f32);
                let last = columns() - 1;
                for x in 0..columns() / 2 {
                    assert_eq!(
                        shape.dots[x],
                        shape.dots[last - x],
                        "{mood:?} at step {step}: column {x} is not mirrored"
                    );
                }
            }
        }
    }

    #[test]
    fn the_scanner_is_symmetric_about_its_core() {
        // It cannot be symmetric about the display's middle, because it crosses it. What it can
        // be -- and what the comet before it was not -- is the same shape either side of its own
        // core, so it looks identical going left as going right. Sampled where the swing puts
        // the core on the middle, which is the one phase where the two tests are the same test.
        for at in [0.25, 0.75] {
            let shape = scanner(at);
            let last = columns() - 1;
            for x in 0..columns() / 2 {
                assert_eq!(
                    shape.dots[x],
                    shape.dots[last - x],
                    "at {at} the core is centred but column {x} is not mirrored"
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
        // these mean -- including `Away`, where the UI is the half that is still alive.
        for mood in EVERY {
            for tick in 0..STEPS {
                let out = strip(mood, tick);
                assert!(out.chars().any(|c| c != '\u{2800}'), "{mood:?} at {tick}");
            }
        }
    }

    #[test]
    fn the_states_do_not_all_draw_the_same_thing() {
        // Told apart at a glance is the requirement, and one pair especially: waiting on the
        // machine and the machine waiting on you must not look alike.
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
            let working = draw(Mood::Working, at);
            let asking = draw(Mood::Asking, at);
            assert_ne!(
                working.dots, asking.dots,
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
    fn the_scanner_slows_into_its_turns() {
        // A linear sweep hits the wall at full speed and reverses in one frame, which nothing
        // physical does. Measured as distance covered: least at the ends, most in the middle.
        let span = columns() as f32 - 1.0;
        let core = |at: f32| swing(at) * span;
        let moved = |at: f32| (core(at + 0.02) - core(at)).abs();
        assert!(
            moved(0.25) > moved(0.0) * 4.0,
            "the middle of the sweep is not much faster than the turn"
        );
        assert!(
            moved(0.25) > moved(0.48) * 4.0,
            "it does not slow into the far end either"
        );
    }

    #[test]
    fn the_scanner_has_a_core_and_a_fringe() {
        // Where the energy is. A block sliding back and forth is a rectangle; what makes this
        // read as a lamp is that it is brightest in one place and falls away from it.
        let shape = scanner(0.25);
        let hottest = shape
            .heat
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(x, _)| x)
            .expect("a lit column");
        for x in 0..columns() {
            let out = x.abs_diff(hottest);
            if out < 2 {
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
    fn the_standing_wave_stays_where_it_is() {
        // The difference from the ripple at a glance: this one does not travel, it gets louder.
        // Its tallest column is the middle at every point in the cycle.
        let middle = columns() / 2;
        for step in 0..STEPS {
            let shape = standing(step as f32 / STEPS as f32);
            let peak = shape
                .heat
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(x, _)| x)
                .expect("a lit column");
            assert!(
                peak.abs_diff(middle) <= 1,
                "step {step}: the peak wandered to {peak}"
            );
        }
    }

    #[test]
    fn the_breath_brightens_as_it_opens() {
        // The heat is the breath itself, not the distance from the middle: a display where the
        // edges are always the cold part does not breathe, it just gets wider.
        let hottest = |shape: &Shape| shape.heat.iter().copied().fold(0.0, f32::max);
        assert!(
            hottest(&breath(0.5)) > hottest(&breath(0.0)),
            "it is no brighter open than shut"
        );
    }

    #[test]
    fn the_break_always_has_a_gap_in_it() {
        // The break moving is what says the UI is still running and it is the other end that is
        // missing. A line with no gap is just a line.
        for step in 1..STEPS {
            let shape = breaking(step as f32 / STEPS as f32);
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
    fn resting_is_slower_than_working() {
        // Two states saying opposite things at the same speed say only that the display is on.
        let (rest_slow, rest_fast) = Mood::Resting.pace();
        let (work_slow, work_fast) = Mood::Working.pace();
        assert!(
            rest_slow * work_fast > work_slow * rest_fast,
            "resting must take longer over one cycle"
        );
    }

    #[test]
    fn a_bar_grows_out_from_the_line_it_starts_on() {
        // Vertically centred rather than growing off the floor: the display is built around a
        // middle, and a bar that grows one way is the same lopsidedness in the other axis.
        let mut shape = Shape::blank();
        shape.light(0, 0.1);
        assert_eq!(
            shape.dots[0].iter().filter(|lit| **lit).count(),
            1,
            "the faintest is one dot"
        );
        shape.light(1, 1.0);
        assert!(
            shape.dots[1].iter().all(|lit| *lit),
            "and the brightest is the whole column"
        );
    }
}
