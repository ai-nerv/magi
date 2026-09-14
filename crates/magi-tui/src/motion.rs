//! How the prompt shows a turn running: a dim band sweeping left to right across the box, in the
//! same column at once on both edges and the words between them — the scanner turned inside out.

use crate::metric;

/// One sweep of the band, from before the left edge to past the right.
const SWEEP_MS: u64 = 2000;

/// Where frame `tick` falls in a sweep, from 0 up to 1.
fn phase(tick: usize) -> f32 {
    let frames = u16::try_from(SWEEP_MS / metric::frame_ms().max(1))
        .unwrap_or(u16::MAX)
        .max(1);
    let at = u16::try_from(tick % usize::from(frames)).unwrap_or(0);
    f32::from(at) / f32::from(frames)
}

/// How deep the band is at column `at` of a box `width` wide: 1 at its middle, 0 outside it, a tenth
/// of the width each side and entering and leaving past the edges. A raised cosine, as Codex's shimmer.
#[must_use]
pub fn band(at: f32, width: f32, tick: usize) -> f32 {
    let half = (width * 0.1).max(3.0);
    let centre = phase(tick) * (width + 2.0 * half) - half;
    let distance = ((at - centre).abs() / half).min(1.0);
    (1.0 + (std::f32::consts::PI * distance).cos()) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The column the band is deepest at on frame `tick`, across a box `width` wide.
    fn deepest(width: u8, tick: usize) -> u8 {
        (0..width)
            .max_by(|a, b| {
                let depth = |column: u8| band(f32::from(column), f32::from(width), tick);
                depth(*a).total_cmp(&depth(*b))
            })
            .unwrap_or(0)
    }

    #[test]
    fn the_band_crosses_the_box_and_nowhere_else() {
        let mut seen = false;
        for tick in 0..100 {
            seen |= (0..30u8).any(|column| band(f32::from(column), 30.0, tick) > 0.9);
            assert!(band(-20.0, 30.0, tick) < f32::EPSILON, "far off the box");
        }
        assert!(seen, "the band never crossed the box");
    }

    #[test]
    fn the_band_sweeps_from_left_to_right() {
        let (early, late) = (deepest(40, 6), deepest(40, 14));
        assert!(early < late, "{early} then {late}");
    }
}
