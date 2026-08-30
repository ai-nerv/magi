//! The opening scramble: letters land as noise and resolve into themselves.
//!
//! Off unless asked for. `axon.ui.decrypt_ms` is how long it runs, and zero — the built-in — is
//! no effect at all, so nobody pays a frame for it who did not want it.
//!
//! ```lua
//! axon.ui.decrypt_ms = 900
//! ```
//!
//! It runs over the finished frame rather than inside each renderer. Every piece of text on the
//! opening screen belongs to a different module — the placeholder, the footer, the status line,
//! a resumed transcript — and threading a progress fraction through all of them to reach a
//! nine-hundred-millisecond flourish is a permanent cost for a temporary thing. A pass over the
//! buffer is one function that nothing else has to know about.
//!
//! **What it will not touch.** The frame the text sits in: box drawing, blocks, and anything
//! that is not one narrow character. Scrambling those turns the prompt box into confetti, and
//! the point of the effect is that the shape is already there while the words arrive.

use ratatui::buffer::Buffer;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// When the UI opened. Unset in a process that never started one, which is every test.
static OPENED: OnceLock<Instant> = OnceLock::new();

/// Start the clock. Only the first call counts.
pub fn begin() {
    let _ = OPENED.set(Instant::now());
}

/// How far through the scramble the screen is, or `None` when there is nothing to do.
#[must_use]
pub fn progress() -> Option<f32> {
    let over = Duration::from_millis(crate::metric::decrypt_ms());
    if over.is_zero() {
        return None;
    }
    let elapsed = OPENED.get()?.elapsed();
    (elapsed < over).then(|| elapsed.as_secs_f32() / over.as_secs_f32())
}

/// Scramble what has not resolved yet, in place.
pub fn over(buffer: &mut Buffer, progress: f32) {
    let tick = (progress * FLICKERS).round() as u64;
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            if !scrambles(cell.symbol()) || resolved(x, y, progress) {
                continue;
            }
            cell.set_symbol(noise(x, y, tick));
        }
    }
}

/// How many times an unresolved cell changes over the whole run.
///
/// Not once per frame: the frame rate is a setting, so tying the flicker to it made the effect
/// frantic on a fast terminal and stately on a slow one for no reason anybody chose.
const FLICKERS: f32 = 18.0;

/// Whether this cell holds something worth scrambling.
///
/// One narrow character, and not part of the furniture. Box drawing (U+2500–U+257F), blocks
/// (U+2580–U+259F) and the geometric shapes above them are the frame, not the message.
fn scrambles(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    !c.is_whitespace() && !matches!(c, '\u{2500}'..='\u{25FF}')
}

/// Whether the cell at `x, y` has landed yet.
///
/// Each cell gets its own threshold rather than resolving left to right, so the screen comes out
/// of the noise all over at once. A wipe reads as a cursor typing; this reads as something
/// settling.
fn resolved(x: u16, y: u16, progress: f32) -> bool {
    let threshold = f32::from(u16::try_from(mix(u64::from(x), u64::from(y)) % 1000).unwrap_or(0))
        / 1000.0
        * SPREAD;
    progress >= threshold
}

/// How much of the run the thresholds are spread over, leaving the rest settled.
const SPREAD: f32 = 0.85;

/// One glyph of noise for a cell on a given tick.
fn noise(x: u16, y: u16, tick: u64) -> &'static str {
    let pool = crate::glyph::decrypt_pool();
    let count = pool.chars().count().max(1);
    let at = usize::try_from(mix(u64::from(x) ^ tick.rotate_left(17), u64::from(y) ^ tick) % 4096)
        .unwrap_or(0)
        % count;
    // A slice of the setting rather than a char, because the pool is a `&'static str` a config
    // may have replaced and a cell wants a string anyway.
    let start = pool.char_indices().nth(at).map_or(0, |(index, _)| index);
    let end = pool
        .char_indices()
        .nth(at + 1)
        .map_or(pool.len(), |(index, _)| index);
    &pool[start..end]
}

/// A cheap deterministic hash, so the same cell scrambles the same way every run.
///
/// Deterministic rather than random because the tests have to be able to say what it drew, and
/// nobody watching can tell the difference between this and entropy.
fn mix(a: u64, b: u64) -> u64 {
    let mut h = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 31;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 29)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    /// A buffer holding `text` on one row, scrambled `progress` of the way through.
    fn scrambled(text: &str, progress: f32) -> String {
        let width = u16::try_from(text.chars().count()).expect("a short line");
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
        buffer.set_string(0, 0, text, ratatui::style::Style::default());
        over(&mut buffer, progress);
        (0..width)
            .map(|x| buffer[(x, 0)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn the_frame_is_left_alone() {
        // Scrambling the box turns the prompt into confetti. The shape is what is already there
        // while the words arrive.
        let box_row = "╭──────────╮";
        assert_eq!(scrambled(box_row, 0.0), box_row);
    }

    #[test]
    fn spaces_stay_spaces() {
        // Otherwise the words run together and the line stops being a line of words.
        let out = scrambled("ab cd ef", 0.0);
        assert_eq!(out.len() - out.trim().len(), 0, "{out}");
        for (at, c) in out.chars().enumerate() {
            assert_eq!(c == ' ', at == 2 || at == 5, "column {at} of {out:?}");
        }
    }

    #[test]
    fn nothing_has_landed_at_the_start_and_everything_has_by_the_end() {
        let text = "ask anything, or / for commands";
        assert_ne!(scrambled(text, 0.0), text, "it starts as noise");
        assert_eq!(scrambled(text, 1.0), text, "and ends as itself");
    }

    #[test]
    fn it_settles_rather_than_wiping() {
        // Each cell has its own threshold, so the screen comes out of the noise all over at
        // once. Left to right would read as a cursor typing, which is a different effect.
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let half = scrambled(text, 0.5);
        let landed: Vec<usize> = half
            .chars()
            .enumerate()
            .filter(|&(_, c)| c == 'a')
            .map(|(at, _)| at)
            .collect();
        assert!(!landed.is_empty() && landed.len() < text.len(), "{half}");
        assert!(
            landed.iter().any(|&at| at > text.len() / 2),
            "nothing landed in the back half: {half}"
        );
        assert!(
            landed.iter().any(|&at| at < text.len() / 2),
            "nor the front: {half}"
        );
    }

    #[test]
    fn the_same_cell_scrambles_the_same_way_every_run() {
        let text = "reproducible";
        assert_eq!(scrambled(text, 0.25), scrambled(text, 0.25));
    }

    #[test]
    fn the_width_never_changes() {
        // A cell is a cell. Anything else moves the text under the box that is framing it.
        for progress in [0.0, 0.3, 0.7, 0.99] {
            assert_eq!(scrambled("hello world", progress).chars().count(), 11);
        }
    }
}
