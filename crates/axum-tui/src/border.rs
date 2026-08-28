//! The prompt's box, and the light that runs around it.
//!
//! The prompt used to be a rule above and a rule below, which is Pi's shape. This is ours: a
//! rounded box, and a scan that travels its border and says what the session is doing without
//! taking a row to say it in.
//!
//! **How the scan works.** The border is addressed as a ring — every cell of it has one index,
//! running clockwise from the top-left corner, so "move the light along by one" is addition
//! rather than four cases for four edges. A scan head sits at some position on that ring, and a
//! cell near it is lit some fraction of the way between the border colour and the scan colour.
//! Two heads on the same ring take the brighter of the two, so they cross without cancelling.
//!
//! **Two things move.** The heads travel, and the whole border *breathes* underneath them — a
//! slow rise and fall of the base colour. The heads say where the light is; the breath says how
//! hard the session is working, and it is what stops a box with nothing happening in it from
//! looking switched off between passes of the scan.
//!
//! **What it says.** The mode is the state, not decoration: at rest two comets drift the ring
//! opposite each other and the breath is slow and shallow; with something typed they leave the
//! circuit and shuttle the long edges in step, which is the shape of a thing waiting to be sent;
//! while a turn runs they race, and the breath goes with them.

use crate::theme::Theme;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// How bright a cell is `n` steps *ahead* of a head, and `n` steps *behind* one.
///
/// A comet rather than a glow. The old table was symmetric — two cells either side, then two
/// dimmer, then out — and a symmetric light does not say which way it is going: at any one frame
/// it is a bright dot, and the motion is only in the difference between frames. A short nose and
/// a long tail read as travel in a single frame, which is what makes the thing look alive
/// standing still.
///
/// Tables rather than a curve, so the shape stays editable by eye.
const NOSE: [f32; 3] = [1.0, 0.5, 0.2];
const TAIL: [f32; 10] = [1.0, 0.9, 0.78, 0.65, 0.52, 0.4, 0.29, 0.2, 0.12, 0.06];

/// One cycle of the breath, as a fraction of the way to the scan colour.
///
/// Asymmetric on purpose: in over five steps, out over eleven. A symmetric pulse reads as a
/// blink, and a border that blinks is a border demanding to be looked at.
const BREATH: [f32; 16] = [
    0.0, 0.05, 0.11, 0.16, 0.19, 0.20, 0.19, 0.17, 0.15, 0.12, 0.10, 0.07, 0.05, 0.03, 0.02, 0.01,
];

/// How many ticks one step of [`BREATH`] lasts, so the pulse rate is the state too.
///
/// Zero is no breath at all.
const fn breath(scan: Scan) -> usize {
    match scan {
        Scan::Resting => 7,
        Scan::Holding => 3,
        Scan::Working => 1,
        Scan::Off => 0,
    }
}

/// Where the breath is in its cycle, as a fraction of the way to the scan colour.
fn breathing(scan: Scan, tick: usize) -> f32 {
    let each = breath(scan);
    if each == 0 {
        return 0.0;
    }
    BREATH[(tick / each) % BREATH.len()]
}

/// A scan head: where it is on the ring, and which way it is travelling.
///
/// The direction is carried rather than derived because a comet has a front and a back, and the
/// one mode that reverses — the shuttle — reverses mid-edge, where nothing about the position
/// alone says which way it just came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Head {
    at: usize,
    forward: bool,
}

/// What the box is doing, which is what the session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    /// Nothing typed, nothing running: two comets drifting the ring, breathing slowly.
    Resting,
    /// Something is in the prompt: two comets shuttling the long edges in step.
    Holding,
    /// A turn is running: the same circuit, at speed, and the breath keeping up.
    Working,
    /// No animation at all.
    Off,
}

/// How far the scan travels per tick, as `cells * num / den`.
///
/// A fraction rather than a divisor so the speeds can sit between whole cells: the caller ticks
/// at the spinner's rate, and one cell per tick is already brisk.
const fn pace(scan: Scan) -> (usize, usize) {
    match scan {
        Scan::Resting => (4, 3),
        Scan::Holding => (2, 1),
        Scan::Working | Scan::Off => (4, 1),
    }
}

/// A box `width` wide holding `rows` of content.
///
/// Answers the top and bottom edges; the sides are [`side`], because the caller owns what goes
/// between them and has to interleave.
#[must_use]
pub fn edges(
    width: u16,
    rows: usize,
    tick: usize,
    scan: Scan,
    theme: &Theme,
) -> (Line<'static>, Line<'static>) {
    let width = usize::from(width).max(2);
    let inner = width - 2;
    let ring = ring_length(inner, rows);
    let heads = heads(scan, tick, inner, rows, ring);
    let base = breathing(scan, tick);

    let mut top = Vec::with_capacity(width);
    top.push(cell('╭', 0, &heads, ring, base, theme));
    for i in 0..inner {
        top.push(cell('─', 1 + i, &heads, ring, base, theme));
    }
    top.push(cell('╮', 1 + inner, &heads, ring, base, theme));

    // Anticlockwise along the bottom, because the ring runs clockwise: the bottom-right corner
    // comes before the bottom-left one when you are walking round.
    let bottom_right = 1 + inner + 1 + rows;
    let mut bottom = Vec::with_capacity(width);
    bottom.push(cell(
        '╰',
        bottom_right + inner + 1,
        &heads,
        ring,
        base,
        theme,
    ));
    for i in 0..inner {
        bottom.push(cell(
            '─',
            bottom_right + inner - i,
            &heads,
            ring,
            base,
            theme,
        ));
    }
    bottom.push(cell('╯', bottom_right, &heads, ring, base, theme));

    (Line::from(top), Line::from(bottom))
}

/// The two side cells for content row `row`, counted from the top.
#[must_use]
pub fn side(
    width: u16,
    rows: usize,
    row: usize,
    tick: usize,
    scan: Scan,
    theme: &Theme,
) -> (Span<'static>, Span<'static>) {
    let inner = usize::from(width).max(2) - 2;
    let ring = ring_length(inner, rows);
    let heads = heads(scan, tick, inner, rows, ring);
    let base = breathing(scan, tick);
    // Right edge runs down after the top-right corner; left edge runs up before the top-left.
    let right = 1 + inner + 1 + row;
    let left = ring - 1 - row;
    (
        cell('│', left, &heads, ring, base, theme),
        cell('│', right, &heads, ring, base, theme),
    )
}

/// How many cells the border has, walking all the way round.
fn ring_length(inner: usize, rows: usize) -> usize {
    // Four corners, two horizontal runs, two vertical runs.
    4 + inner * 2 + rows * 2
}

/// Where the light is, in ring coordinates.
///
/// **Always two.** One head reads as a stray highlight; two read as a mechanism. What differs
/// between the modes is what the pair is doing — running the ring opposite each other, or
/// shuttling the long edges in step — and how fast.
fn heads(scan: Scan, tick: usize, inner: usize, rows: usize, ring: usize) -> Vec<Head> {
    if ring == 0 {
        return Vec::new();
    }
    let (num, den) = pace(scan);
    let step = tick * num / den;
    let forward = |at: usize| Head { at, forward: true };
    match scan {
        Scan::Off => Vec::new(),
        // Opposite points of the ring, so the box always has light on two sides of it. Resting
        // and working are the same figure at different speeds, which is the honest relationship
        // between them: nothing is happening, or something is, and it is the same box either way.
        Scan::Resting | Scan::Working => {
            vec![forward(step % ring), forward((step + ring / 2) % ring)]
        }
        // The two long edges, swept in step and reversing at the ends: a shuttle rather than a
        // circuit, because something is waiting to be sent rather than travelling.
        //
        // The bottom edge is walked anticlockwise -- the ring runs clockwise, so its leftmost
        // cell is its *highest* index. Column `at` on the bottom is therefore
        // `bottom_right + inner - at`, and getting that off by one put the lower light a cell
        // ahead of the upper one for the whole sweep.
        Scan::Holding => {
            let span = inner.max(1);
            let at = bounce(step, span).min(inner.saturating_sub(1));
            let out = rising(step, span);
            let bottom_right = 1 + inner + 1 + rows;
            // Mirrored directions, because the two are walking the same screen columns from
            // opposite ends of the ring: the pair moves left together and the tails must too.
            vec![
                Head {
                    at: 1 + at,
                    forward: out,
                },
                Head {
                    at: bottom_right + inner - at,
                    forward: !out,
                },
            ]
        }
    }
}

/// A position that walks up to `span - 1` and back down again, forever.
fn bounce(step: usize, span: usize) -> usize {
    if span <= 1 {
        return 0;
    }
    let period = (span - 1) * 2;
    let at = step % period;
    if at < span { at } else { period - at }
}

/// Whether a [`bounce`] is on its way out or on its way back.
fn rising(step: usize, span: usize) -> bool {
    if span <= 1 {
        return true;
    }
    let period = (span - 1) * 2;
    step % period < span - 1
}

/// How brightly one head lights the cell at `at`.
///
/// The nose is measured in the direction of travel and the tail against it, so the same head
/// running the other way lights the other side of itself.
fn lit(at: usize, head: Head, ring: usize) -> f32 {
    if ring == 0 {
        return 0.0;
    }
    let at = at % ring;
    let head_at = head.at % ring;
    let clockwise = (at + ring - head_at) % ring;
    let anticlockwise = (head_at + ring - at) % ring;
    let (nose, tail) = if head.forward {
        (clockwise, anticlockwise)
    } else {
        (anticlockwise, clockwise)
    };
    let ahead = NOSE.get(nose).copied().unwrap_or(0.0);
    let behind = TAIL.get(tail).copied().unwrap_or(0.0);
    ahead.max(behind)
}

/// One border cell: the breath underneath, and whichever head lights it most.
fn cell(
    glyph: char,
    at: usize,
    heads: &[Head],
    ring: usize,
    base: f32,
    theme: &Theme,
) -> Span<'static> {
    let mut best = base;
    for &head in heads {
        best = best.max(lit(at, head, ring));
    }
    let colour = if best > 0.0 {
        mix(theme.border, theme.border_scan, best)
    } else {
        theme.border
    };
    Span::styled(glyph.to_string(), Style::default().fg(colour))
}

/// `amount` of the way from `from` to `to`.
fn mix(from: Color, to: Color, amount: f32) -> Color {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) = (from, to) else {
        // A themed colour that is not RGB cannot be interpolated; the scan simply does not show
        // rather than the border changing to something unrelated.
        return from;
    };
    let blend = |a: u8, b: u8| {
        let a = f32::from(a);
        let b = f32::from(b);
        (a + (b - a) * amount).round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(blend(fr, tr), blend(fg, tg), blend(fb, tb))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_box_is_rounded_and_spans_the_width() {
        let (top, bottom) = edges(20, 1, 0, Scan::Off, &crate::theme::DARK);
        assert_eq!(text(&top), "╭──────────────────╮");
        assert_eq!(text(&bottom), "╰──────────────────╯");
    }

    #[test]
    fn the_sides_are_bars() {
        let (l, r) = side(20, 1, 0, 0, Scan::Off, &crate::theme::DARK);
        assert_eq!(l.content.as_ref(), "│");
        assert_eq!(r.content.as_ref(), "│");
    }

    #[test]
    fn with_the_scan_off_every_cell_is_the_border_colour() {
        let (top, _) = edges(20, 1, 7, Scan::Off, &crate::theme::DARK);
        assert!(
            top.spans
                .iter()
                .all(|s| s.style.fg == Some(crate::theme::DARK.border)),
            "nothing is lit"
        );
    }

    #[test]
    fn every_running_mode_has_two_heads() {
        // One reads as a stray highlight; two read as a mechanism.
        let theme = crate::theme::DARK;
        for scan in [Scan::Resting, Scan::Holding, Scan::Working] {
            let (top, bottom) = edges(40, 1, 0, scan, &theme);
            let peaks = top
                .spans
                .iter()
                .chain(bottom.spans.iter())
                .filter(|s| s.style.fg == Some(theme.border_scan))
                .count();
            assert_eq!(peaks, 2, "{scan:?} on a one-row box puts both on the edges");
        }
    }

    #[test]
    fn resting_and_working_are_the_same_figure_at_different_speeds() {
        // Nothing is happening, or something is, and it is the same box either way.
        assert_eq!(
            heads(Scan::Resting, 30, 20, 1, 44),
            heads(Scan::Working, 10, 20, 1, 44),
            "resting at 2/3 and working at 2/1 land together when the ticks line up"
        );
    }

    #[test]
    fn the_light_falls_off_with_distance() {
        // A gradient, not a single lit cell.
        let head = Head {
            at: 40,
            forward: true,
        };
        let ring = 100;
        assert!((lit(40, head, ring) - 1.0).abs() < f32::EPSILON, "the head");
        for step in 1..TAIL.len() {
            assert!(
                lit(40 - step, head, ring) < lit(40 - step + 1, head, ring),
                "the tail dims as it goes back, at {step}"
            );
        }
        assert_eq!(lit(40 - TAIL.len(), head, ring), 0.0, "and then it is out");
    }

    #[test]
    fn a_head_trails_behind_itself_rather_than_glowing_evenly() {
        // The comet, and the reason for it: a symmetric light says nothing about which way it
        // is going, so in any single frame it is a bright dot rather than something moving.
        let ring = 100;
        let head = Head {
            at: 40,
            forward: true,
        };
        assert!(
            lit(35, head, ring) > lit(45, head, ring),
            "the tail is the long side"
        );
        let back = Head {
            at: 40,
            forward: false,
        };
        assert!(
            lit(45, back, ring) > lit(35, back, ring),
            "and it swaps sides when the head turns round"
        );
    }

    #[test]
    fn a_tail_that_reaches_the_corner_carries_on_round() {
        // Ring arithmetic, not four edges: a comet crossing the top-left corner keeps its tail.
        let ring = 60;
        let head = Head {
            at: 2,
            forward: true,
        };
        assert!(lit(58, head, ring) > 0.0, "the tail wrapped past zero");
    }

    #[test]
    fn the_border_breathes_under_the_scan() {
        // What stops a box with nothing happening in it looking switched off between passes.
        let cycle: Vec<f32> = (0..BREATH.len() * breath(Scan::Working))
            .map(|tick| breathing(Scan::Working, tick))
            .collect();
        let high = cycle.iter().copied().fold(0.0_f32, f32::max);
        assert!(high > 0.1, "it rises: {high}");
        assert!(cycle.contains(&0.0), "and falls back");
        assert_eq!(breathing(Scan::Off, 7), 0.0, "off is off");
    }

    #[test]
    fn a_working_box_breathes_faster_than_a_resting_one() {
        // The rate is the state, the same way the pace is.
        assert!(breath(Scan::Working) < breath(Scan::Holding));
        assert!(breath(Scan::Holding) < breath(Scan::Resting));
    }

    #[test]
    fn the_scan_moves_with_the_tick() {
        let theme = crate::theme::DARK;
        let a = text_colours(&edges(30, 1, 0, Scan::Resting, &theme).0);
        let b = text_colours(&edges(30, 1, 12, Scan::Resting, &theme).0);
        assert_ne!(a, b, "it travels");
    }

    fn text_colours(line: &Line<'_>) -> Vec<Option<Color>> {
        line.spans.iter().map(|s| s.style.fg).collect()
    }

    #[test]
    fn holding_lights_both_long_edges() {
        // Two heads sweeping in step: the shape of something waiting to be sent.
        let theme = crate::theme::DARK;
        let (top, bottom) = edges(30, 1, 0, Scan::Holding, &theme);
        assert!(top.spans.iter().any(|s| s.style.fg != Some(theme.border)));
        assert!(
            bottom
                .spans
                .iter()
                .any(|s| s.style.fg != Some(theme.border))
        );
    }

    #[test]
    fn a_bounce_turns_round_rather_than_wrapping() {
        let seen: Vec<usize> = (0..8).map(|s| bounce(s, 5)).collect();
        assert_eq!(seen, vec![0, 1, 2, 3, 4, 3, 2, 1]);
    }

    #[test]
    fn a_narrow_box_does_not_panic() {
        for width in 0..6_u16 {
            let _ = edges(width, 1, 3, Scan::Working, &crate::theme::DARK);
            let _ = side(width, 1, 0, 3, Scan::Working, &crate::theme::DARK);
        }
    }

    #[test]
    fn a_tall_box_lights_its_sides_too() {
        // Swept across ticks rather than pinned to one: which cell is lit at a given tick is a
        // function of the pace, and pinning it made a speed change look like a bug in the ring.
        let theme = crate::theme::DARK;
        let lit = (0..120)
            .flat_map(|tick| (0..6).map(move |row| (tick, row)))
            .filter(|&(tick, row)| {
                let (l, r) = side(30, 6, row, tick, Scan::Working, &theme);
                l.style.fg != Some(theme.border) || r.style.fg != Some(theme.border)
            })
            .count();
        assert!(lit > 0, "the scan goes round, not just along the top");
    }
}

#[cfg(test)]
mod holding_tests {
    use super::*;

    /// Which screen column of a rendered edge is brightest.
    fn peak(line: &Line<'_>, theme: &Theme) -> Option<usize> {
        line.spans
            .iter()
            .position(|s| s.style.fg == Some(theme.border_scan))
    }

    #[test]
    fn the_two_lights_stay_in_the_same_column() {
        // The bottom edge is walked anticlockwise, so its leftmost cell is its highest ring
        // index. Getting that off by one put the lower light a cell ahead for the whole sweep.
        let theme = crate::theme::DARK;
        for tick in 0..60 {
            let (top, bottom) = edges(40, 1, tick, Scan::Holding, &theme);
            let (Some(t), Some(b)) = (peak(&top, &theme), peak(&bottom, &theme)) else {
                continue;
            };
            assert_eq!(t, b, "tick {tick}: top at {t}, bottom at {b}");
        }
    }

    #[test]
    fn both_lights_are_present_from_the_first_tick() {
        let theme = crate::theme::DARK;
        let (top, bottom) = edges(40, 1, 0, Scan::Holding, &theme);
        assert!(peak(&top, &theme).is_some(), "top lit at rest");
        assert!(peak(&bottom, &theme).is_some(), "and so is the bottom");
    }

    #[test]
    fn the_sweep_turns_round_inside_the_edge() {
        // It must not walk onto a corner and wrap: this is a shuttle, not a circuit.
        let theme = crate::theme::DARK;
        let width = 20u16;
        for tick in 0..80 {
            let (top, _) = edges(width, 1, tick, Scan::Holding, &theme);
            if let Some(at) = peak(&top, &theme) {
                assert!(at >= 1, "tick {tick}: on the left corner");
                assert!(
                    at <= usize::from(width) - 2,
                    "tick {tick}: on the right corner"
                );
            }
        }
    }

    #[test]
    fn every_mode_moves_faster_than_it_did() {
        // Doubled twice. Two thirds of a cell per tick read as drifting rather than scanning;
        // the pass after that still read as slow next to the spinner it ticks with.
        assert_eq!(pace(Scan::Resting), (4, 3));
        assert_eq!(pace(Scan::Holding), (2, 1));
        assert_eq!(pace(Scan::Working), (4, 1));
    }
}
