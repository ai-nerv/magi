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
//! **Only the heads move.** The whole border used to breathe underneath them as well — a slow rise
//! and fall of the base colour, a couple of steps up the ramp and back. In 24-bit colour that was
//! a few values of grey. On a palette index it is not: a step along this ramp is a step towards
//! the accent, so the breath recoloured the entire box on a cycle, and what it read as was a
//! border that could not decide what colour it was. Anything the border says, it says with the
//! heads.
//!
//! **What it says.** The mode is the state, not decoration: at rest two comets drift the ring
//! opposite each other; with something typed they leave the circuit and shuttle the long edges in
//! step, which is the shape of a thing waiting to be sent; while a turn runs they race.

use crate::colour;
use crate::glyph;
use crate::metric;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// How bright a cell is `n` steps *ahead* of a head, and `n` steps *behind* one.
///
/// A comet rather than a glow. The old table was symmetric — two cells either side, then two
/// dimmer, then out — and a symmetric light does not say which way it is going: at any one frame
/// it is a bright dot, and the motion is only in the difference between frames. A short nose and
/// a long tail read as travel in a single frame, which is what makes the thing look alive
/// standing still.
///
/// A curve rather than a table, now that the two lengths are settings: a table cannot be as long
/// as somebody asks for. Squared, so the fade is quick near the head and slow out at the end,
/// which is what the hand-written table was.
fn fade(step: u16, over: u16) -> f32 {
    if step >= over {
        return 0.0;
    }
    let left = f32::from(over - step) / f32::from(over.max(1));
    left * left
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
    /// Nothing typed, nothing running: two comets drifting the ring, slowly.
    Resting,
    /// Something is in the prompt: two comets shuttling the long edges in step.
    Holding,
    /// A turn is running: the same circuit, at speed.
    Working,
    /// No animation at all.
    Off,
}

/// How far the scan travels per tick, as `cells * num / den`.
///
/// A fraction rather than a divisor so the speeds can sit between whole cells: the caller ticks
/// at the spinner's rate, and one cell per tick is already brisk.
fn pace(scan: Scan) -> (usize, usize) {
    let hundredths = match scan {
        Scan::Resting => metric::rest_pace(),
        Scan::Holding => metric::hold_pace(),
        Scan::Working | Scan::Off => metric::work_pace(),
    };
    (usize::from(hundredths), 100)
}

/// A box `width` wide holding `rows` of content.
///
/// Answers the top and bottom edges; the sides are [`side`], because the caller owns what goes
/// between them and has to interleave.
#[must_use]
pub fn edges(width: u16, rows: usize, tick: usize, scan: Scan) -> (Line<'static>, Line<'static>) {
    let width = usize::from(width).max(2);
    let inner = width - 2;
    let ring = ring_length(inner, rows);
    let heads = heads(scan, tick, inner, rows, ring);

    let mut top = Vec::with_capacity(width);
    top.push(cell(glyph::corner_top_left(), 0, &heads, ring));
    for i in 0..inner {
        top.push(cell(glyph::edge_horizontal(), 1 + i, &heads, ring));
    }
    top.push(cell(glyph::corner_top_right(), 1 + inner, &heads, ring));

    // Anticlockwise along the bottom, because the ring runs clockwise: the bottom-right corner
    // comes before the bottom-left one when you are walking round.
    let bottom_right = 1 + inner + 1 + rows;
    let mut bottom = Vec::with_capacity(width);
    bottom.push(cell(
        glyph::corner_bottom_left(),
        bottom_right + inner + 1,
        &heads,
        ring,
    ));
    for i in 0..inner {
        bottom.push(cell(
            glyph::edge_horizontal(),
            bottom_right + inner - i,
            &heads,
            ring,
        ));
    }
    bottom.push(cell(
        glyph::corner_bottom_right(),
        bottom_right,
        &heads,
        ring,
    ));

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
) -> (Span<'static>, Span<'static>) {
    let inner = usize::from(width).max(2) - 2;
    let ring = ring_length(inner, rows);
    let heads = heads(scan, tick, inner, rows, ring);
    // Right edge runs down after the top-right corner; left edge runs up before the top-left.
    let right = 1 + inner + 1 + row;
    let left = ring - 1 - row;
    (
        cell(glyph::edge_vertical(), left, &heads, ring),
        cell(glyph::edge_vertical(), right, &heads, ring),
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
    let ahead = fade(u16::try_from(nose).unwrap_or(u16::MAX), metric::scan_nose());
    let behind = fade(u16::try_from(tail).unwrap_or(u16::MAX), metric::scan_tail());
    ahead.max(behind)
}

/// One border cell, lit by whichever head is nearest.
fn cell(glyph: &str, at: usize, heads: &[Head], ring: usize) -> Span<'static> {
    let mut best = 0.0_f32;
    for &head in heads {
        best = best.max(lit(at, head, ring));
    }
    Span::styled(
        glyph.to_string(),
        Style::default().fg(colour::scan_at(best)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_box_is_rounded_and_spans_the_width() {
        let (top, bottom) = edges(20, 1, 0, Scan::Off);
        assert_eq!(text(&top), "╭──────────────────╮");
        assert_eq!(text(&bottom), "╰──────────────────╯");
    }

    #[test]
    fn the_sides_are_bars() {
        let (l, r) = side(20, 1, 0, 0, Scan::Off);
        assert_eq!(l.content.as_ref(), "│");
        assert_eq!(r.content.as_ref(), "│");
    }

    #[test]
    fn with_the_scan_off_every_cell_is_the_border_colour() {
        let (top, _) = edges(20, 1, 7, Scan::Off);
        assert!(
            top.spans
                .iter()
                .all(|s| s.style.fg == Some(colour::border())),
            "nothing is lit"
        );
    }

    #[test]
    fn every_running_mode_has_two_heads() {
        // One reads as a stray highlight; two read as a mechanism.
        for scan in [Scan::Resting, Scan::Holding, Scan::Working] {
            let (top, bottom) = edges(40, 1, 0, scan);
            let peaks = top
                .spans
                .iter()
                .chain(bottom.spans.iter())
                .filter(|s| s.style.fg == Some(colour::scan_at(1.0)))
                .count();
            assert_eq!(peaks, 2, "{scan:?} on a one-row box puts both on the edges");
        }
    }

    #[test]
    fn resting_and_working_are_the_same_figure_at_different_speeds() {
        // Nothing is happening, or something is, and it is the same box either way. The figure
        // is what is asserted rather than a tick either mode lands on: the two paces are settings
        // now, and a test that pins them to a ratio fails the moment somebody changes one.
        let ring = 44;
        for scan in [Scan::Resting, Scan::Working] {
            let heads = heads(scan, 30, 20, 1, ring);
            assert_eq!(heads.len(), 2, "{scan:?}");
            let apart = heads[1].at.abs_diff(heads[0].at);
            assert_eq!(apart, ring / 2, "{scan:?}: opposite each other");
            assert!(heads.iter().all(|h| h.forward), "{scan:?}: both travelling");
        }
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
        let tail = usize::from(metric::scan_tail());
        for step in 1..tail {
            assert!(
                lit(40 - step, head, ring) < lit(40 - step + 1, head, ring),
                "the tail dims as it goes back, at {step}"
            );
        }
        assert_eq!(lit(40 - tail, head, ring), 0.0, "and then it is out");
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
    fn a_cell_no_head_is_near_is_the_resting_border() {
        // The border does not move on its own. It used to breathe — the base colour rising and
        // falling a couple of steps — which in 24-bit colour was a few values of grey and on a
        // palette index is a walk towards the accent, so the box changed colour rather than
        // brightness.
        let quiet: Vec<_> = (0..40)
            .map(|tick| {
                let (top, _) = edges(60, 1, tick, Scan::Working);
                top.spans[30].style.fg
            })
            .collect();
        assert!(
            quiet.contains(&Some(colour::border())),
            "an unlit cell is the border colour, whatever the tick"
        );
    }

    #[test]
    fn the_scan_moves_with_the_tick() {
        let a = text_colours(&edges(30, 1, 0, Scan::Resting).0);
        let b = text_colours(&edges(30, 1, 12, Scan::Resting).0);
        assert_ne!(a, b, "it travels");
    }

    fn text_colours(line: &Line<'_>) -> Vec<Option<ratatui::style::Color>> {
        line.spans.iter().map(|s| s.style.fg).collect()
    }

    #[test]
    fn holding_lights_both_long_edges() {
        // Two heads sweeping in step: the shape of something waiting to be sent.
        let (top, bottom) = edges(30, 1, 0, Scan::Holding);
        assert!(
            top.spans
                .iter()
                .any(|s| s.style.fg != Some(colour::border()))
        );
        assert!(
            bottom
                .spans
                .iter()
                .any(|s| s.style.fg != Some(colour::border()))
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
            let _ = edges(width, 1, 3, Scan::Working);
            let _ = side(width, 1, 0, 3, Scan::Working);
        }
    }

    #[test]
    fn a_tall_box_lights_its_sides_too() {
        // Swept across ticks rather than pinned to one: which cell is lit at a given tick is a
        // function of the pace, and pinning it made a speed change look like a bug in the ring.
        let lit = (0..120)
            .flat_map(|tick| (0..6).map(move |row| (tick, row)))
            .filter(|&(tick, row)| {
                let (l, r) = side(30, 6, row, tick, Scan::Working);
                l.style.fg != Some(colour::border()) || r.style.fg != Some(colour::border())
            })
            .count();
        assert!(lit > 0, "the scan goes round, not just along the top");
    }
}

#[cfg(test)]
mod holding_tests {
    use super::*;

    /// Which screen column of a rendered edge is brightest.
    fn peak(line: &Line<'_>) -> Option<usize> {
        line.spans
            .iter()
            .position(|s| s.style.fg == Some(colour::scan_at(1.0)))
    }

    #[test]
    fn the_two_lights_stay_in_the_same_column() {
        // The bottom edge is walked anticlockwise, so its leftmost cell is its highest ring
        // index. Getting that off by one put the lower light a cell ahead for the whole sweep.
        for tick in 0..60 {
            let (top, bottom) = edges(40, 1, tick, Scan::Holding);
            let (Some(t), Some(b)) = (peak(&top), peak(&bottom)) else {
                continue;
            };
            assert_eq!(t, b, "tick {tick}: top at {t}, bottom at {b}");
        }
    }

    #[test]
    fn both_lights_are_present_from_the_first_tick() {
        let (top, bottom) = edges(40, 1, 0, Scan::Holding);
        assert!(peak(&top).is_some(), "top lit at rest");
        assert!(peak(&bottom).is_some(), "and so is the bottom");
    }

    #[test]
    fn the_sweep_turns_round_inside_the_edge() {
        // It must not walk onto a corner and wrap: this is a shuttle, not a circuit.
        let width = 20u16;
        for tick in 0..80 {
            let (top, _) = edges(width, 1, tick, Scan::Holding);
            if let Some(at) = peak(&top) {
                assert!(at >= 1, "tick {tick}: on the left corner");
                assert!(
                    at <= usize::from(width) - 2,
                    "tick {tick}: on the right corner"
                );
            }
        }
    }

    #[test]
    fn the_modes_are_paced_against_each_other() {
        // The ordering is the point and the numbers are settings: drift at rest, shuttle with
        // something waiting, race while a turn runs. A config that inverts it will get what it
        // asked for, but the built-in set says what the shape is meant to be.
        let cells = |scan| {
            let (num, den) = pace(scan);
            num * 1000 / den
        };
        assert!(cells(Scan::Resting) < cells(Scan::Holding));
        assert!(cells(Scan::Holding) < cells(Scan::Working));
        assert!(
            cells(Scan::Resting) > 1000,
            "and even resting is a cell a tick"
        );
    }
}
