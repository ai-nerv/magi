//! The opening scramble: letters land as noise and resolve into themselves. Off unless asked for —
//! `magi.ui.decrypt_ms` is how long it runs, and zero, the built-in, is no effect at all. It runs
//! over the finished frame rather than inside each renderer, and will not touch the frame the text
//! sits in: box drawing, blocks, anything that is not one narrow character.
//!
//! ```lua
//! magi.ui.decrypt_ms = 900
//! ```

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
pub fn over(buffer: &mut Buffer, area: ratatui::layout::Rect, progress: f32) {
    let tick = (progress * FLICKERS).round() as u64;
    let area = area.intersection(buffer.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            if !scrambles(cell.symbol()) || resolved(x, y, progress) {
                continue;
            }
            cell.set_symbol(noise(x, y, tick, crate::glyph::decrypt_pool()));
        }
    }
}

/// A scramble that can start again, for something that opens after the session has: a clock the
/// caller holds, keyed on *which* thing is open, since a list refilters on every keystroke and
/// re-scrambling as you type is a fault. Two different lists are two openings.
#[derive(Debug, Default)]
pub struct Landing {
    at: Option<Instant>,
    was: Option<String>,
}

impl Landing {
    pub fn showing(&mut self, what: Option<&str>) {
        if self.was.as_deref() == what {
            return;
        }
        self.was = what.map(ToOwned::to_owned);
        self.at = what.map(|_| Instant::now());
    }

    #[must_use]
    pub fn progress(&self) -> Option<f32> {
        let over = Duration::from_millis(crate::metric::decrypt_ms());
        if over.is_zero() {
            return None;
        }
        let elapsed = self.at?.elapsed();
        (elapsed < over).then(|| elapsed.as_secs_f32() / over.as_secs_f32())
    }
}

/// How many times an unresolved cell changes over the whole run. Not once per frame: the frame rate
/// is a setting, and tying the flicker to it made the effect frantic on a fast terminal.
const FLICKERS: f32 = 18.0;

/// Whether this cell holds something worth scrambling: one narrow character, and not box drawing
/// (U+2500-U+257F), blocks (U+2580-U+259F) or the geometric shapes above them.
fn scrambles(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    !c.is_whitespace() && !matches!(c, '\u{2500}'..='\u{25FF}')
}

/// Whether the cell at `x, y` has landed yet. Each cell gets its own threshold rather than resolving
/// left to right, so the screen comes out of the noise all over at once.
fn resolved(x: u16, y: u16, progress: f32) -> bool {
    let threshold = f32::from(u16::try_from(mix(u64::from(x), u64::from(y)) % 1000).unwrap_or(0))
        / 1000.0
        * SPREAD;
    progress >= threshold
}

/// How much of the run the thresholds are spread over, leaving the rest settled.
const SPREAD: f32 = 0.85;

/// One glyph of noise for a cell on a given tick.
fn noise(x: u16, y: u16, tick: u64, pool: &'static str) -> &'static str {
    let count = pool.chars().count().max(1);
    let at = usize::try_from(mix(u64::from(x) ^ tick.rotate_left(17), u64::from(y) ^ tick) % 4096)
        .unwrap_or(0)
        % count;
    // A slice of the setting rather than a char: the pool is a `&'static str` a config may have replaced.
    let start = pool.char_indices().nth(at).map_or(0, |(index, _)| index);
    let end = pool
        .char_indices()
        .nth(at + 1)
        .map_or(pool.len(), |(index, _)| index);
    &pool[start..end]
}

/// Glitch the occasional character inside `area`, briefly: a letter goes to a symbol for a beat and
/// comes back as itself. Confined to an area because a glitch in a tool result is indistinguishable
/// from a tool that printed one. Off unless `magi.ui.flicker_odds` says otherwise, and skipped while
/// the opening scramble is still running.
pub fn flicker(buffer: &mut Buffer, area: ratatui::layout::Rect) {
    let odds = u64::from(crate::metric::flicker_odds());
    if odds == 0 || progress().is_some() {
        return;
    }
    let Some(opened) = OPENED.get() else {
        return;
    };
    // Quantised so a glitch holds for a beat instead of strobing: every frame in a window rolls alike.
    let held = crate::metric::flicker_ms().max(1);
    let window = u64::try_from(opened.elapsed().as_millis()).unwrap_or(0) / held;
    let area = area.intersection(buffer.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if !glitched(x, y, window, odds) {
                continue;
            }
            let cell = &mut buffer[(x, y)];
            if !scrambles(cell.symbol()) {
                continue;
            }
            cell.set_symbol(noise(x, y, window, crate::glyph::flicker_pool()));
        }
    }
}

fn glitched(x: u16, y: u16, window: u64, odds: u64) -> bool {
    mix(
        u64::from(x) ^ window.wrapping_mul(31),
        u64::from(y) ^ window,
    )
    .is_multiple_of(odds)
}

/// A cheap deterministic hash, so the same cell scrambles the same way every run and the tests can
/// say what it drew.
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

    fn scrambled(text: &str, progress: f32) -> String {
        let width = u16::try_from(text.chars().count()).expect("a short line");
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
        buffer.set_string(0, 0, text, ratatui::style::Style::default());
        let area = buffer.area;
        over(&mut buffer, area, progress);
        (0..width)
            .map(|x| buffer[(x, 0)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn the_frame_is_left_alone() {
        // Scrambling the box turns the prompt into confetti.
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
        // Each cell has its own threshold; left to right would read as a cursor typing.
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

/// The box never quite settles: the occasional character glitches and comes back.
#[cfg(test)]
mod flicker_tests {
    use super::*;

    fn glitching(width: u16, window: u64, odds: u64) -> Vec<u16> {
        (0..width)
            .filter(|&x| glitched(x, 0, window, odds))
            .collect()
    }

    #[test]
    fn off_is_off() {
        // Zero odds is the built-in, and it has to mean nothing happens rather than everything.
        assert_eq!(BUILT_IN_ODDS, 0);
    }

    const BUILT_IN_ODDS: u16 = crate::metric::BUILT_IN.flicker_odds;

    #[test]
    fn it_is_rare() {
        // A tenth of the box at once is not interference, it is a fault.
        let hits: usize = (0..200)
            .map(|window| glitching(60, window, 250).len())
            .sum();
        assert!(hits > 0, "nothing ever glitched");
        assert!(hits < 200 * 60 / 20, "far too many: {hits} of {}", 200 * 60);
    }

    #[test]
    fn a_glitch_holds_for_its_whole_window() {
        // Quantised: every frame inside one window rolls the same way.
        let first = glitching(60, 7, 40);
        assert_eq!(first, glitching(60, 7, 40));
    }

    #[test]
    fn and_then_moves_on() {
        // Windows either side of one another must not agree, or the glitch is a dead pixel.
        let same: usize = (0..64)
            .filter(|&window| glitching(60, window, 40) == glitching(60, window + 1, 40))
            .count();
        assert!(same < 32, "it is stuck: {same} windows in 64 repeated");
    }

    #[test]
    fn what_it_glitches_to_is_a_symbol_not_a_letter() {
        let pool = crate::glyph::flicker_pool();
        assert!(
            !pool.chars().any(char::is_alphanumeric),
            "a letter turning into a letter reads as the text changing: {pool:?}"
        );
    }
}

/// A list opening is its own scramble, and the same list narrowing is not.
#[cfg(test)]
mod landing_tests {
    use super::*;

    #[test]
    fn nothing_open_lands_nothing() {
        let mut landing = Landing::default();
        landing.showing(None);
        assert!(landing.at.is_none(), "there is nothing to scramble");
    }

    #[test]
    fn the_same_list_twice_running_only_starts_once() {
        let mut landing = Landing::default();
        landing.showing(Some("model"));
        let started = landing.at.expect("a list opened");
        landing.showing(Some("model"));
        assert_eq!(
            landing.at.expect("still open"),
            started,
            "narrowing a list is not opening one"
        );
    }

    #[test]
    fn a_different_list_starts_again() {
        let mut landing = Landing::default();
        landing.showing(Some("model"));
        let started = landing.at.expect("a list opened");
        landing.showing(Some("allow this?"));
        assert!(
            landing.at.expect("still open") > started,
            "a permission ask after a model list is a second opening"
        );
    }

    #[test]
    fn closing_a_list_stops_the_clock() {
        let mut landing = Landing::default();
        landing.showing(Some("model"));
        landing.showing(None);
        assert!(landing.at.is_none(), "nothing is open to land");
        assert!(landing.progress().is_none(), "so there is no progress");
    }
}
