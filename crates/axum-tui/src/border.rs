//! The prompt's box, and the light that runs around it.
//!
//! The prompt used to be a rule above and a rule below, which is Pi's shape. This is ours: a
//! rounded box, and a scan that travels its border and says what the session is doing without
//! taking a row to say it in.
//!
//! **How the scan works.** The border is addressed as a ring — every cell of it has one index,
//! running clockwise from the top-left corner, so "move the light along by one" is addition
//! rather than four cases for four edges. A scan head sits at some position on that ring; a cell
//! `n` steps away from it is lit at `FALLOFF[n]` of the way between the border colour and the
//! scan colour, and past the end of that table it is the border colour. Two heads on the same
//! ring take the brighter of the two, so they cross without cancelling.
//!
//! **What it says.** The mode is the state, not decoration: at rest one head drifts the whole
//! ring; with something typed two heads sweep the long edges in step, which is the shape of a
//! thing waiting to be sent; while a turn runs they chase each other around, faster.

use crate::theme::Theme;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// How bright a cell is at each distance from a scan head, as a fraction of the way from the
/// border colour to the scan colour.
///
/// The head, then two either side a little less, then two less again, then out. Reading it as a
/// table rather than computing it keeps the shape editable by eye.
const FALLOFF: [f32; 6] = [1.0, 0.62, 0.62, 0.3, 0.3, 0.12];

/// What the box is doing, which is what the session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    /// Nothing typed, nothing running: one head drifting the whole ring.
    Resting,
    /// Something is in the prompt: two heads sweeping the long edges together.
    Holding,
    /// A turn is running: two heads chasing each other around, quicker.
    Working,
    /// No animation at all.
    Off,
}

/// Ticks per step, per mode. Higher is slower; the caller ticks at the spinner's rate.
const fn pace(scan: Scan) -> usize {
    match scan {
        Scan::Resting => 3,
        Scan::Holding => 2,
        Scan::Working | Scan::Off => 1,
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

    let mut top = Vec::with_capacity(width);
    top.push(cell('╭', 0, &heads, ring, theme));
    for i in 0..inner {
        top.push(cell('─', 1 + i, &heads, ring, theme));
    }
    top.push(cell('╮', 1 + inner, &heads, ring, theme));

    // Anticlockwise along the bottom, because the ring runs clockwise: the bottom-right corner
    // comes before the bottom-left one when you are walking round.
    let bottom_right = 1 + inner + 1 + rows;
    let mut bottom = Vec::with_capacity(width);
    bottom.push(cell('╰', bottom_right + inner + 1, &heads, ring, theme));
    for i in 0..inner {
        bottom.push(cell('─', bottom_right + inner - i, &heads, ring, theme));
    }
    bottom.push(cell('╯', bottom_right, &heads, ring, theme));

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
    // Right edge runs down after the top-right corner; left edge runs up before the top-left.
    let right = 1 + inner + 1 + row;
    let left = ring - 1 - row;
    (
        cell('│', left, &heads, ring, theme),
        cell('│', right, &heads, ring, theme),
    )
}

/// How many cells the border has, walking all the way round.
fn ring_length(inner: usize, rows: usize) -> usize {
    // Four corners, two horizontal runs, two vertical runs.
    4 + inner * 2 + rows * 2
}

/// Where the light is, in ring coordinates.
fn heads(scan: Scan, tick: usize, inner: usize, rows: usize, ring: usize) -> Vec<usize> {
    if ring == 0 {
        return Vec::new();
    }
    let step = tick / pace(scan);
    match scan {
        Scan::Off => Vec::new(),
        Scan::Resting => vec![step % ring],
        // The two long edges, swept in step and reversing at the ends: a shuttle rather than a
        // circuit, because something is waiting to be sent rather than travelling.
        Scan::Holding => {
            let span = inner.max(1);
            let at = bounce(step, span);
            vec![
                1 + at,
                1 + inner + 1 + rows + (inner - 1 - at.min(inner - 1)),
            ]
        }
        // Opposite points of the ring, so the box always has light on two sides of it.
        Scan::Working => vec![step % ring, (step + ring / 2) % ring],
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

/// One border cell, lit by whichever head is nearest.
fn cell(glyph: char, at: usize, heads: &[usize], ring: usize, theme: &Theme) -> Span<'static> {
    let mut best = 0.0_f32;
    for &head in heads {
        let d = around(at, head, ring);
        if let Some(&lit) = FALLOFF.get(d) {
            best = best.max(lit);
        }
    }
    let colour = if best > 0.0 {
        mix(theme.border, theme.border_scan, best)
    } else {
        theme.border
    };
    Span::styled(glyph.to_string(), Style::default().fg(colour))
}

/// Distance between two points on a ring, the short way round.
fn around(a: usize, b: usize, ring: usize) -> usize {
    if ring == 0 {
        return usize::MAX;
    }
    let d = a.abs_diff(b);
    d.min(ring - d)
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
    fn resting_lights_exactly_one_brightest_cell() {
        let theme = crate::theme::DARK;
        let (top, bottom) = edges(30, 1, 0, Scan::Resting, &theme);
        let brightest = top
            .spans
            .iter()
            .chain(bottom.spans.iter())
            .filter(|s| s.style.fg == Some(theme.border_scan))
            .count();
        assert_eq!(brightest, 1, "one head, one peak");
    }

    #[test]
    fn the_light_falls_off_with_distance() {
        // The point of the table: a gradient, not a single lit cell.
        let theme = crate::theme::DARK;
        let (top, _) = edges(30, 1, 0, Scan::Resting, &theme);
        let lit = top
            .spans
            .iter()
            .filter(|s| s.style.fg != Some(theme.border))
            .count();
        assert!(
            lit > 1 && lit <= FALLOFF.len() * 2,
            "a tail, not a dot: {lit}"
        );
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
    fn a_ring_distance_takes_the_short_way() {
        assert_eq!(around(0, 9, 10), 1);
        assert_eq!(around(9, 0, 10), 1);
        assert_eq!(around(0, 5, 10), 5);
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
        let theme = crate::theme::DARK;
        let lit = (0..6)
            .map(|row| side(30, 6, row, 40, Scan::Working, &theme))
            .filter(|(l, r)| l.style.fg != Some(theme.border) || r.style.fg != Some(theme.border))
            .count();
        assert!(lit > 0, "the scan goes round, not just along the top");
    }
}
