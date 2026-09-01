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
    /// A list or a permission is open and it is your move. A breath, in and out.
    Asking,
    /// The daemon is not there. A flat line with the signal dropping out of it.
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
    fn blank(columns: usize) -> Self {
        Self {
            dots: vec![[false; ROWS]; columns],
            heat: vec![0.0; columns],
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
fn draw(mood: Mood, phase: f32, columns: usize) -> Shape {
    match mood {
        Mood::Resting => wave(phase, 0.8, 1.2, columns),
        Mood::Holding => wave(phase, 1.6, 1.5, columns),
        Mood::Working => scanner(phase, columns),
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

/// A scanner crossing and coming back, with a fringe either side of its core.
///
/// The one everybody knows, and the one thing on this display that was replaced rather than
/// kept. It was a comet: a head with its tail on one side, which is a shape that looks right
/// going one way and wrong going the other, and grew a hard triangular edge as it went. This is
/// symmetric about its own core, so it looks the same in both directions and only the direction
/// changes.
///
/// Eased by [`swing`], which is the other half of it: a linear sweep arrives at the wall at full
/// speed and reverses in a single frame, and nothing physical does that. This one slows into
/// each turn and comes back out of it.
fn scanner(phase: f32, columns: usize) -> Shape {
    let mut shape = Shape::blank(columns);
    let span = columns as f32 - 1.0;
    let core = swing(phase) * span;
    let reach = (columns as f32 / 4.0).max(1.5);
    for x in 0..columns {
        let away = (x as f32 - core).abs();
        // Linear falloff from the core, the same in both directions: a core and a fringe rather
        // than a block with a hard edge, which is what makes it read as a lamp.
        shape.light(x, (1.0 - away / reach).max(0.0));
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
    fn the_scanner_is_symmetric_about_its_core() {
        // The one thing that changed, and the reason it changed. The comet before it had its
        // tail on one side, which looks right going one way and wrong going the other. Sampled
        // where the swing puts the core on the middle, so the display's own middle is the core's.
        for at in [0.25, 0.75] {
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
    }

    #[test]
    fn the_scanner_slows_into_its_turns() {
        // A linear sweep hits the wall at full speed and reverses in one frame, which nothing
        // physical does. Measured as distance covered: least at the ends, most in the middle.
        let span = COLUMNS as f32 - 1.0;
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
        let shape = scanner(0.25, COLUMNS);
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
