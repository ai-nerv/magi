//! The prompt's two rules, and the light that runs along them. The box has no corners and no
//! sides: what is drawn is a rule above the text and a rule below it, and the text between them
//! has the full width to itself.
//!
//! They are still addressed as a ring — left to right along the top, then right to left along the
//! bottom, so the light circulates rather than jumping back — and a cell near a scan head is lit
//! some fraction of the way from border colour to scan colour; two heads take the brighter. Only
//! the heads move: a step along this ramp is a step towards the accent, so a base colour that moved
//! recoloured the whole box. The mode is the state: drifting at rest, shuttling the two rules with
//! something typed, and while a turn runs no heads at all — a dim band sweeps across the box.

use crate::colour;
use crate::glyph;
use crate::metric;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// How bright a cell is `n` steps ahead of a head, and `n` steps behind one. A short nose and a long
/// tail read as travel in a single frame, where a symmetric light is only a bright dot. Squared, so
/// the fade is quick near the head and slow at the end, and a curve because the lengths are settings.
fn fade(step: u16, over: u16) -> f32 {
    if step >= over {
        return 0.0;
    }
    let left = f32::from(over - step) / f32::from(over.max(1));
    left * left
}

/// A scan head: where it is on the ring, and which way it is travelling. The direction is carried
/// rather than derived because the shuttle reverses mid-edge, where the position alone says nothing.
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
    /// A turn is running: two dim bands bounce across the box, both edges in step.
    Working,
    /// A floating pane has the keyboard: four comets, evenly spaced. Four because it is a bigger box
    /// and two heads on a border twice the length leave most of it unlit.
    Focused,
    Off,
}

/// How far the scan travels per tick, as `cells * num / den`. A fraction so speeds can sit between
/// whole cells: the caller ticks at the spinner's rate and one cell per tick is already brisk.
fn pace(scan: Scan) -> (usize, usize) {
    let hundredths = match scan {
        // A pane drifts at the resting pace: it holds the keyboard, it is not doing work.
        Scan::Resting | Scan::Focused => metric::rest_pace(),
        Scan::Holding => metric::hold_pace(),
        Scan::Working | Scan::Off => metric::work_pace(),
    };
    (usize::from(hundredths), 100)
}

/// The rule above the text and the rule below it, each `width` across.
///
/// `rows` is what the box holds, and no longer reaches the light: with nothing drawn down the
/// sides, how tall the box is says nothing about how far the light has to travel.
#[must_use]
pub fn edges(width: u16, tick: usize, scan: Scan) -> (Line<'static>, Line<'static>) {
    let width = usize::from(width).max(1);
    let ring = ring_length(width);
    // Working sweeps one band across by column, so both rules dim in step.
    if matches!(scan, Scan::Working) {
        let rule = || {
            Line::from(
                (0..width)
                    .map(|i| shaded(glyph::edge_horizontal(), i, width, tick))
                    .collect::<Vec<_>>(),
            )
        };
        return (rule(), rule());
    }
    let heads = heads(scan, tick, width, ring);
    let bright = |at: usize| from_heads(at, &heads, ring);

    let top = (0..width)
        .map(|i| paint(glyph::edge_horizontal(), bright(i)))
        .collect::<Vec<_>>();
    // Right to left along the bottom, so the light comes back the way a circuit would.
    let bottom = (0..width)
        .map(|i| paint(glyph::edge_horizontal(), bright(bottom_at(i, width))))
        .collect::<Vec<_>>();

    (Line::from(top), Line::from(bottom))
}

/// Where column `column` of the bottom rule sits on the ring.
fn bottom_at(column: usize, width: usize) -> usize {
    width + (width - 1 - column.min(width - 1))
}

/// How many cells the light travels: the top rule, then the bottom one.
fn ring_length(width: usize) -> usize {
    width * 2
}

/// Where the light is, in ring coordinates. Always two: one head reads as a stray highlight.
fn heads(scan: Scan, tick: usize, width: usize, ring: usize) -> Vec<Head> {
    if ring == 0 {
        return Vec::new();
    }
    let (num, den) = pace(scan);
    let step = tick * num / den;
    let forward = |at: usize| Head { at, forward: true };
    match scan {
        // Working does not walk the ring at all — its band sweeps by column, drawn by [`shadowed`],
        // not by heads. Off is dark. Both light nothing here.
        Scan::Off | Scan::Working => Vec::new(),
        // Opposite points of the ring, so the box always has light on two sides of it.
        Scan::Resting => {
            vec![forward(step % ring), forward((step + ring / 2) % ring)]
        }
        // Evenly spaced, so the gaps between them are equal however long the border is.
        Scan::Focused => (0..4)
            .map(|nth| forward((step + ring * nth / 4) % ring))
            .collect(),
        // The two rules, swept in step and reversing at the ends: a shuttle, not a circuit. The
        // bottom is walked right to left, so mirrored directions put both heads in the same
        // screen column while they travel opposite ways round the ring.
        Scan::Holding => {
            let span = width.max(1);
            let at = bounce(step, span).min(width.saturating_sub(1));
            let out = rising(step, span);
            vec![
                Head { at, forward: out },
                Head {
                    at: bottom_at(at, width),
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

/// How brightly one head lights the cell at `at`. The nose is measured in the direction of travel
/// and the tail against it, so the same head running the other way lights the other side of itself.
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

/// How brightly the heads light the cell at ring position `at`.
fn from_heads(at: usize, heads: &[Head], ring: usize) -> f32 {
    heads
        .iter()
        .fold(0.0_f32, |best, &head| best.max(lit(at, head, ring)))
}

/// A glyph painted a fraction of the way from border colour to scan colour.
fn paint(glyph: &str, best: f32) -> Span<'static> {
    Span::styled(
        glyph.to_string(),
        Style::default().fg(colour::scan_at(best)),
    )
}

/// The working border at `column` of a box `width` across: the resting border colour, only ever
/// darkened, where the band is — the same band the words in the box carry.
fn shaded(glyph: &str, column: usize, width: usize, tick: usize) -> Span<'static> {
    let dimmed = dimming(column, width, tick);
    Span::styled(
        glyph.to_owned(),
        colour::shade(colour::border(), colour::shimmer_shadow(), dimmed),
    )
}

/// How far the band has dimmed `column` of a box `width` across, from 0 lit to 1 dark. Asked by
/// column, so both edges, the words between them and what sits on the border all dim together.
#[must_use]
pub fn dimming(column: usize, width: usize, tick: usize) -> f32 {
    let cell = |n: usize| f32::from(u16::try_from(n).unwrap_or(u16::MAX));
    crate::motion::band(cell(column) + 0.5, cell(width), tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn the_box_is_two_rules_across_the_whole_width() {
        let (top, bottom) = edges(20, 0, Scan::Off);
        assert_eq!(text(&top), "────────────────────");
        assert_eq!(text(&bottom), "────────────────────");
    }

    #[test]
    fn working_dims_both_edges_in_the_same_columns_and_sweeps() {
        // One band across the box: the top and bottom edges match column for column, and it moves.
        let width = 24u16;
        let colours = |line: &Line<'_>| line.spans.iter().map(|s| s.style.fg).collect::<Vec<_>>();
        let top_at = |tick| colours(&edges(width, tick, Scan::Working).0);
        for tick in 0..60 {
            let (top, bottom) = edges(width, tick, Scan::Working);
            assert_eq!(colours(&top), colours(&bottom), "out of step at {tick}");
        }
        assert!(
            (0..60).any(|tick| top_at(tick).windows(2).any(|p| p[0] != p[1])),
            "no band ever showed"
        );
        assert_ne!(top_at(3), top_at(9), "the band stood still");
    }

    #[test]
    fn working_never_lights_the_border_it_only_darkens_it() {
        // And stays in the terminal's own palette, as the scan does: a themed grey is never swapped
        // for a fixed one.
        use ratatui::style::Color;
        let Color::Indexed(ceiling) = colour::border() else {
            panic!("the border is a palette grey");
        };
        let Color::Indexed(floor) = colour::shimmer_shadow() else {
            panic!("and so is its shadow");
        };
        for tick in 0..60 {
            let (top, bottom) = edges(24, tick, Scan::Working);
            for cell in top.spans.iter().chain(&bottom.spans) {
                let Some(Color::Indexed(n)) = cell.style.fg else {
                    panic!("left the palette: {cell:?}");
                };
                assert!(
                    (floor..=ceiling).contains(&n),
                    "{n} is not between the border and its shadow at {tick}"
                );
            }
        }
    }

    #[test]
    fn what_sits_on_the_border_dims_between_lit_and_dark() {
        for tick in 0..60 {
            let dim = dimming(4, 30, tick);
            assert!((0.0..=1.0).contains(&dim), "{dim} at {tick}");
        }
    }

    #[test]
    fn with_the_scan_off_every_cell_is_the_border_colour() {
        let (top, _) = edges(20, 7, Scan::Off);
        assert!(
            top.spans
                .iter()
                .all(|s| s.style.fg == Some(colour::border())),
            "nothing is lit"
        );
    }

    #[test]
    fn every_running_mode_has_two_heads() {
        for scan in [Scan::Resting, Scan::Holding] {
            let (top, bottom) = edges(40, 0, scan);
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
    fn resting_is_two_comets_opposite_on_the_ring() {
        // The figure is asserted rather than a tick, because the pace is a setting. Working is no
        // longer this figure — it sweeps the edges (see the working tests), so only rest is checked.
        let ring = ring_length(20);
        let heads = heads(Scan::Resting, 30, 20, ring);
        assert_eq!(heads.len(), 2);
        assert_eq!(
            heads[1].at.abs_diff(heads[0].at),
            ring / 2,
            "opposite each other"
        );
        assert!(heads.iter().all(|h| h.forward), "both travelling");
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
        // A symmetric light says nothing about which way it is going.
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
        // The border does not move on its own: a base colour that moved would walk towards the accent.
        let quiet: Vec<_> = (0..40)
            .map(|tick| {
                let (top, _) = edges(60, tick, Scan::Resting);
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
        let a = text_colours(&edges(30, 0, Scan::Resting).0);
        let b = text_colours(&edges(30, 12, Scan::Resting).0);
        assert_ne!(a, b, "it travels");
    }

    fn text_colours(line: &Line<'_>) -> Vec<Option<ratatui::style::Color>> {
        line.spans.iter().map(|s| s.style.fg).collect()
    }

    #[test]
    fn holding_lights_both_long_edges() {
        // Two heads sweeping in step: the shape of something waiting to be sent.
        let (top, bottom) = edges(30, 0, Scan::Holding);
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
            for scan in [Scan::Working, Scan::Resting, Scan::Holding, Scan::Focused] {
                let _ = edges(width, 3, scan);
            }
        }
    }

    #[test]
    fn the_light_reaches_the_bottom_rule_as_well_as_the_top() {
        // The ring circulates: along the top, then back along the bottom. Checked on `Resting`;
        // `Working` has no ring at all, its band sweeps by column.
        let lit = (0..200)
            .filter(|&tick| {
                let (_, bottom) = edges(30, tick, Scan::Resting);
                bottom
                    .spans
                    .iter()
                    .any(|s| s.style.fg != Some(colour::border()))
            })
            .count();
        assert!(lit > 0, "the scan comes back along the bottom");
    }
}

#[cfg(test)]
mod holding_tests {
    use super::*;

    fn peak(line: &Line<'_>) -> Option<usize> {
        line.spans
            .iter()
            .position(|s| s.style.fg == Some(colour::scan_at(1.0)))
    }

    #[test]
    fn the_two_lights_stay_in_the_same_column() {
        // The bottom edge is walked anticlockwise, so its leftmost cell is its highest ring index.
        for tick in 0..60 {
            let (top, bottom) = edges(40, tick, Scan::Holding);
            let (Some(t), Some(b)) = (peak(&top), peak(&bottom)) else {
                continue;
            };
            assert_eq!(t, b, "tick {tick}: top at {t}, bottom at {b}");
        }
    }

    #[test]
    fn both_lights_are_present_from_the_first_tick() {
        let (top, bottom) = edges(40, 0, Scan::Holding);
        assert!(peak(&top).is_some(), "top lit at rest");
        assert!(peak(&bottom).is_some(), "and so is the bottom");
    }

    #[test]
    fn the_sweep_uses_the_whole_rule_and_turns_round_at_the_ends() {
        // With no corners to avoid, the shuttle owns every column — and still reverses rather
        // than wrapping, which is what makes it a shuttle and not a circuit.
        let width = 20u16;
        let mut seen = Vec::new();
        for tick in 0..80 {
            let (top, _) = edges(width, tick, Scan::Holding);
            if let Some(at) = peak(&top) {
                assert!(at < usize::from(width), "tick {tick}: off the end");
                seen.push(at);
            }
        }
        // Within a cell of each end: the pace is a setting and steps more than one cell a tick,
        // so which exact column it lands on is not the point. The corners used to cost it two.
        let (first, last) = (
            seen.iter().min().copied().expect("lit somewhere"),
            seen.iter().max().copied().expect("lit somewhere"),
        );
        assert!(first <= 1, "never reached the left end: {first}");
        assert!(
            last >= usize::from(width) - 2,
            "never reached the right end: {last}"
        );
    }

    #[test]
    fn the_modes_are_paced_against_each_other() {
        // The ordering is the point and the numbers are settings: drift, shuttle, race.
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

#[cfg(test)]
mod ring {
    use super::*;

    #[test]
    fn the_ring_is_the_two_rules_and_nothing_else() {
        // No corners and no sides, so the light travels exactly twice the width.
        for width in [1_usize, 10, 78, 200] {
            assert_eq!(ring_length(width), width * 2, "{width}");
        }
    }

    #[test]
    fn the_bottom_is_walked_right_to_left_so_the_circuit_closes() {
        let width = 10;
        // The top runs 0..width left to right; the bottom picks up where it left off, at the
        // right-hand end, and runs back — so stepping off the end of one lands on the start of
        // the other rather than jumping the width of the box.
        assert_eq!(
            bottom_at(width - 1, width),
            width,
            "just after the top ends"
        );
        assert_eq!(
            bottom_at(0, width),
            ring_length(width) - 1,
            "and the last cell is the bottom-left"
        );
        let walked: Vec<usize> = (0..width).map(|c| bottom_at(c, width)).collect();
        let mut sorted = walked.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), width, "every column has its own cell");
    }

    #[test]
    fn a_focused_border_wears_four_lights_evenly_spaced() {
        // Four rather than two: a pane is a bigger box, and two heads would leave most of it dark.
        let width = 78;
        let ring = ring_length(width);
        let lights = heads(Scan::Focused, 0, width, ring);
        assert_eq!(lights.len(), 4);
        let gaps: Vec<usize> = lights.windows(2).map(|p| p[1].at - p[0].at).collect();
        for gap in &gaps {
            assert!(
                gap.abs_diff(ring / 4) <= 1,
                "evenly spaced round {ring}: {gaps:?}"
            );
        }
    }

    #[test]
    fn the_prompt_still_wears_two() {
        let ring = ring_length(20);
        assert_eq!(heads(Scan::Resting, 0, 20, ring).len(), 2);
        assert_eq!(heads(Scan::Holding, 0, 20, ring).len(), 2);
        // Working rides no heads: its band sweeps by column, drawn by `shaded`, not the ring.
        assert!(heads(Scan::Working, 0, 20, ring).is_empty());
        assert!(heads(Scan::Off, 0, 20, ring).is_empty());
    }
}
