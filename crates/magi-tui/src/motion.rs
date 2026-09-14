//! How the prompt shows a turn running: two dim bands bouncing across the box, in the same column
//! at once on both edges and the words between them — the scanner turned inside out.

use crate::metric;

/// One crossing of the box. The bands go there and back, so a whole cycle is two of these.
const SWEEP_MS: u64 = 6000;

/// Where frame `tick` falls in a cycle, from 0 up to 1.
fn phase(tick: usize) -> f32 {
    let frames = u16::try_from(2 * SWEEP_MS / metric::frame_ms().max(1))
        .unwrap_or(u16::MAX)
        .max(1);
    let at = u16::try_from(tick % usize::from(frames)).unwrap_or(0);
    f32::from(at) / f32::from(frames)
}

/// How deep the bands are at column `at` of a box `width` wide: 1 at either one's middle, 0 outside
/// both, a tenth of the width each side. One starts at each edge and bounces off the far one rather
/// than starting again, so they meet in the middle twice a cycle. Raised cosines, as Codex's shimmer.
#[must_use]
pub fn band(at: f32, width: f32, tick: usize) -> f32 {
    let half = (width * 0.1).max(3.0);
    let there = 1.0 - (2.0 * phase(tick) - 1.0).abs();
    let depth = |centre: f32| {
        let distance = ((at - centre).abs() / half).min(1.0);
        (1.0 + (std::f32::consts::PI * distance).cos()) / 2.0
    };
    depth(there * width).max(depth((1.0 - there) * width))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn two_bands_start_at_the_edges_meet_twice_and_bounce_back() {
        let cycle = usize::try_from(2 * SWEEP_MS / metric::frame_ms()).expect("frames");
        let depth = |column: f32, tick: usize| band(column, 40.0, tick);
        assert!(
            depth(0.0, 0) > 0.9 && depth(40.0, 0) > 0.9,
            "one at each edge"
        );
        assert!(depth(20.0, cycle / 4) > 0.9, "they meet on the way over");
        assert!(depth(0.0, cycle / 2) > 0.9, "and bounce off the far edges");
        assert!(
            depth(20.0, cycle * 3 / 4) > 0.9,
            "and meet again on the way back"
        );
        assert!(
            depth(20.0, 0) < f32::EPSILON,
            "nothing in the middle at the start"
        );
    }
}
