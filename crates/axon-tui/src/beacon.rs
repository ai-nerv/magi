//! Five cells under the box that say what the session is doing.
//!
//! A word said it before — `waiting` — and a word is a poor instrument for this. It is read
//! once and then never again, it says nothing about *how long*, and every state that is not
//! "working" looked identical to every other. What replaced it first was one character fading
//! down a ramp, which at the built-in frame interval had three steps to work with and came out
//! as a blink.
//!
//! So: braille. Five cells are ten dot columns by four dot rows, which is a forty-pixel display
//! in a little more than the width of the word it replaces. Each state draws its own thing on
//! it, and they are meant to be told apart at a glance without being read — the way you know a
//! machine is on from across the room.
//!
//! **Every column also carries a heat**, and the cells are coloured from it rather than all
//! together. The dots say the shape and the colour says where the energy in it is: the head of
//! a comet is bright and its tail falls away to the border grey, a breath brightens as it
//! opens. One colour for the whole strip threw that away and left the dots doing all the work.
//!
//! Nothing here is a clock. Every animation is a phase from the frame counter and the two
//! settings that bracket it, so a state runs at the rate `axon.ui.beacon_ms` asks for whatever
//! the frame rate happens to be.

use crate::colour;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// How many cells wide the display is.
pub const CELLS: usize = 5;

/// Dot columns across it: two to a cell.
const COLUMNS: usize = CELLS * 2;

/// Dot rows down it.
const ROWS: usize = 4;

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
    /// A turn is running. A comet, at speed.
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
            Self::Working => (1, 3),
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
/// the tail of a comet, the far edge of a breath — and drawing that with one colour for the
/// whole strip is what made the first version of this read as a progress bar.
struct Shape {
    /// Lit dots, by column and then row. Row zero is the top.
    dots: [[bool; ROWS]; COLUMNS],
    /// How much is going on in each column, from 0.0 to 1.0.
    heat: [f32; COLUMNS],
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            dots: [[false; ROWS]; COLUMNS],
            heat: [0.0; COLUMNS],
        }
    }
}

/// The display as it stands this frame.
#[must_use]
pub fn render(mood: Mood, tick: usize) -> Vec<Span<'static>> {
    let shape = draw(mood, phase(mood, tick));
    let ramp = mood.ramp();
    (0..CELLS)
        .map(|cell| {
            // A cell is two columns and one colour, so it takes the hotter of the two rather
            // than their average: a comet's head landing in the right half of a cell should
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

/// What this state draws at this point in its cycle.
///
/// Split from [`render`] so a test can say what a state draws without going through spans and
/// colours to find out.
fn draw(mood: Mood, phase: f32) -> Shape {
    match mood {
        Mood::Resting => wave(phase, 0.8, 1.2),
        Mood::Holding => wave(phase, 1.6, 1.5),
        Mood::Working => comet(phase),
        Mood::Asking => breath(phase),
        Mood::Away => dropout(phase),
    }
}

/// A sine drifting right, `height` rows tall and `waves` of it across the display.
///
/// Drawn as a joined line rather than a dot per column: the same reason an oscilloscope draws
/// one. Unjoined, a wave with any real amplitude is a scatter of dots that reads as noise. The
/// heat is how high the line has climbed, so the crests are the lit part and the troughs sit
/// back in the border grey — which is what gives a wave only two rows tall anything to read.
fn wave(phase: f32, height: f32, waves: f32) -> Shape {
    let at = |x: usize| {
        let turn = phase + x as f32 / COLUMNS as f32 * waves;
        (turn * std::f32::consts::TAU).sin()
    };
    let middle = (ROWS as f32 - 1.0) / 2.0;
    let mut shape = trace(|x| middle + at(x) * height / 2.0);
    for x in 0..COLUMNS {
        shape.heat[x] = at(x).mul_add(0.5, 0.5);
    }
    shape
}

/// A head sweeping left to right with a tail behind it, as a bar per column.
///
/// The bars grow from the bottom, so the shape reads as something passing rather than a row of
/// lights coming on: a full column at the head, then shorter ones behind it. The heat falls away
/// over the same distance, so the tail cools as well as shortens.
fn comet(phase: f32) -> Shape {
    let mut shape = Shape::default();
    let head = phase * COLUMNS as f32;
    let tail = ROWS as f32;
    for x in 0..COLUMNS {
        // Measured the long way round as well, so the tail follows the head over the wrap
        // instead of being cut off at the right edge every cycle.
        let behind = (head - x as f32 + COLUMNS as f32) % COLUMNS as f32;
        if behind >= tail {
            continue;
        }
        let tall = ROWS - behind as usize;
        for row in 0..tall {
            shape.dots[x][ROWS - 1 - row] = true;
        }
        shape.heat[x] = 1.0 - behind / tail;
    }
    shape
}

/// A bar in the middle breathing out to the edges and back.
///
/// Symmetric on purpose. Everything else here travels, and a thing that grows in place is the
/// one shape that does not read as progress — which is the point: nothing is progressing, it is
/// your move. The heat is the breath itself rather than the distance from the middle, so the
/// whole display brightens as it opens instead of the edges always being the cold part.
fn breath(phase: f32) -> Shape {
    let mut shape = Shape::default();
    let open = (phase * std::f32::consts::TAU).cos().mul_add(-0.5, 0.5);
    // Never all the way shut. A frame with nothing lit reads as the UI having died, which is
    // the one thing none of these states mean -- so it bottoms out at the two middle columns.
    let reach = (open * COLUMNS as f32 / 2.0).max(0.5);
    let middle = (COLUMNS as f32 - 1.0) / 2.0;
    for x in 0..COLUMNS {
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
fn dropout(phase: f32) -> Shape {
    let mut shape = Shape::default();
    let gap = (phase * COLUMNS as f32) as usize % COLUMNS;
    let after = (gap + 1) % COLUMNS;
    for x in 0..COLUMNS {
        if x == gap || x == after {
            continue;
        }
        shape.dots[x][ROWS - 2] = true;
        let beside = x == (gap + COLUMNS - 1) % COLUMNS || x == (after + 1) % COLUMNS;
        shape.heat[x] = if beside { 1.0 } else { 0.0 };
    }
    shape
}

/// Light a joined line through the height `at` gives for each column.
fn trace(at: impl Fn(usize) -> f32) -> Shape {
    let mut shape = Shape::default();
    let row_of = |height: f32| {
        let clamped = height.round().clamp(0.0, ROWS as f32 - 1.0);
        ROWS - 1 - clamped as usize
    };
    for x in 0..COLUMNS {
        let here = row_of(at(x));
        let previous = row_of(at(if x == 0 { COLUMNS - 1 } else { x - 1 }));
        for row in here.min(previous)..=here.max(previous) {
            shape.dots[x][row] = true;
        }
    }
    shape
}

/// One braille cell of the display.
fn cell_of(dots: &[[bool; ROWS]; COLUMNS], cell: usize) -> char {
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

/// Every state draws something, it moves, it is coloured by where the energy in it is, and it
/// is always five cells of braille.
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
    fn it_is_always_five_cells_of_braille() {
        for mood in EVERY {
            for tick in 0..96 {
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
                (1..96).any(|tick| strip(mood, tick) != first),
                "{mood:?} never moved off {first:?}"
            );
        }
    }

    #[test]
    fn every_state_has_something_lit_at_every_moment() {
        // An empty frame reads as the UI having died, which is the opposite of what any of
        // these mean -- including `Away`, where the UI is the half that is still alive.
        for mood in EVERY {
            for tick in 0..96 {
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
                    (0..96).any(|tick| strip(*mood, tick) != strip(*other, tick)),
                    "{mood:?} and {other:?} draw the same thing throughout"
                );
            }
        }
    }

    #[test]
    fn the_cells_are_not_all_one_colour() {
        // The whole reason a column carries a heat. One colour across the strip leaves the dots
        // doing all the work, and a comet whose tail is as bright as its head is a bar chart.
        for mood in EVERY {
            assert!(
                (0..96).any(|tick| {
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
                (0..96).any(|tick| colours(mood, tick).contains(&Some(hottest))),
                "{mood:?} never reaches {hottest:?}"
            );
        }
    }

    #[test]
    fn the_comet_is_hottest_at_its_head() {
        // Where the energy is. A tail that cools as it shortens is the difference between
        // something passing and a row of lights going out.
        let shape = comet(0.0);
        assert!(shape.heat[0] > 0.9, "the head is hot: {:?}", shape.heat);
        for behind in 1..ROWS {
            let x = (COLUMNS - behind) % COLUMNS;
            let ahead = (x + 1) % COLUMNS;
            assert!(
                shape.heat[x] < shape.heat[ahead],
                "column {x} is not cooler than the one ahead of it: {:?}",
                shape.heat
            );
        }
    }

    #[test]
    fn the_comet_has_a_head_and_a_tail() {
        // Not a row of lights coming on: the head is a full column and what is behind it is
        // shorter. Walked backwards from the head with the wrap, because the tail follows it
        // round rather than being cut off at the edge.
        let shape = comet(0.0);
        let tall = |x: usize| shape.dots[x].iter().filter(|lit| **lit).count();
        assert_eq!(tall(0), ROWS, "the head is full height");
        for back in 1..COLUMNS {
            let behind = (COLUMNS - back) % COLUMNS;
            assert!(
                tall(behind) <= tall((behind + 1) % COLUMNS),
                "column {behind} is taller than the one ahead of it"
            );
        }
    }

    #[test]
    fn the_breath_is_symmetric() {
        // It grows in place rather than travelling, which is the one shape here that does not
        // read as progress -- because nothing is progressing, it is your move.
        for step in 0..16 {
            let shape = breath(step as f32 / 16.0);
            for x in 0..COLUMNS / 2 {
                assert_eq!(
                    shape.dots[x],
                    shape.dots[COLUMNS - 1 - x],
                    "step {step} column {x} is not mirrored"
                );
            }
        }
    }

    #[test]
    fn the_breath_brightens_as_it_opens() {
        // The heat is the breath itself, not the distance from the middle: a display where the
        // edges are always the cold part does not breathe, it just gets wider.
        let shut = breath(0.0);
        let open = breath(0.5);
        let hottest = |shape: &Shape| shape.heat.iter().copied().fold(0.0, f32::max);
        assert!(
            hottest(&open) > hottest(&shut),
            "{:?} is not brighter than {:?}",
            open.heat,
            shut.heat
        );
    }

    #[test]
    fn the_dropout_always_has_a_gap_in_it() {
        // The gap moving is what says the UI is still running and it is the other end that is
        // missing. A line with no gap is just a line.
        for step in 0..16 {
            let shape = dropout(step as f32 / 16.0);
            assert!(
                (0..COLUMNS).any(|x| shape.dots[x].iter().all(|lit| !lit)),
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
        let shape = wave(0.0, 3.0, 1.5);
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
