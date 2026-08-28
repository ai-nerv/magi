//! The colours, which are the terminal's own.
//!
//! There is no theme. There was one — Pi's `dark.json`, ported hex by hex — and every value in it
//! was a decision about what somebody's terminal should look like, made by somebody who has never
//! seen it. A person who has set their terminal's palette has already answered that question, and
//! answering it again over the top is how a program ends up being the one window on the screen
//! that does not match the others.
//!
//! So every colour here is an **index into the palette the terminal already has**, and what those
//! indices actually look like is not this program's business.
//!
//! **The defaults assume nothing.** They are the ordinary xterm reading: 1 is red, 2 is green, 3
//! is yellow, 6 is cyan, and 232-255 is the 24-step greyscale. That is what somebody who has never
//! configured anything has, and it is what they get.
//!
//! **A generated palette is a different machine.** [lule](https://github.com/bresilla/lule) builds
//! one from a wallpaper: its slots 1-6 are the six most chromatic pigments of an image in chroma
//! order — no hue meaning at all, and `accent` and `cursor` both alias slot 1 — and its 232-255 is
//! not a greyscale but `black → colour 0 → accent → colour 15 → white`, which makes the middle of
//! that range the accent rather than a grey. Every one of those is a reason to move a role
//! somewhere else, and none of them is a reason to make everybody else live with it. So the
//! defaults stay ordinary and the whole set is settable from Lua:
//!
//! ```lua
//! axum.ui.accent = 1
//! axum.ui.muted  = 8
//! ```
//!
//! See `config/init.lua` for the full list and for a lule preset.

use ratatui::style::Color;
use std::sync::OnceLock;

/// Every colour the UI draws with, as palette indices.
///
/// One value read once at startup rather than a parameter threaded through every render
/// function: a palette that can change mid-frame is a palette two halves of one screen can
/// disagree about, and nothing wants that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Spinners, list cursors, markdown bullets, inline code.
    pub accent: u8,
    /// Success states, added diff lines, a tool that finished.
    pub success: u8,
    /// Warnings and elevated context usage.
    pub warning: u8,
    /// Errors, removed diff lines, a tool that failed.
    pub error: u8,
    /// Markdown headings.
    pub heading: u8,
    /// Fenced code blocks.
    pub code: u8,
    /// The characters you have already typed, wherever they appear in a candidate.
    pub match_: u8,
    /// Behind a tool block and behind a menu, so each reads as one object.
    pub block_bg: u8,
    /// Behind a user message, and behind the row of a list you are on.
    pub raised_bg: u8,
    /// The rule above and below the prompt.
    pub rule: u8,
    /// Tertiary text; the footer lives here.
    pub dim: u8,
    /// Secondary text: tool output, quotes, reasoning, unchanged diff lines.
    pub muted: u8,
    /// Default foreground.
    pub text: u8,
    /// The row of a list you are on, which has to beat [`Palette::text`] to read as selected.
    pub selected: u8,
    /// The prompt's border with nothing lit, and the floor of its scan.
    pub border: u8,
    /// The brightest point of the scan travelling along the border.
    ///
    /// The scan is the run of indices between this and [`Palette::border`] rather than a blend of
    /// the two: blending needs to know what the colours *are*, which is exactly what this design
    /// has given up knowing, and a contiguous run of palette indices is already a gradient.
    pub scan: u8,
}

/// What a terminal nobody has configured looks like.
///
/// Chosen against the standard xterm palette and close to the hex values this replaced: the
/// accent was a teal, the border a dark slate, the block backgrounds two steps off black.
pub const STOCK: Palette = Palette {
    accent: 6,
    success: 2,
    warning: 3,
    error: 1,
    heading: 11,
    code: 2,
    match_: 14,
    block_bg: 234,
    raised_bg: 237,
    rule: 239,
    dim: 241,
    muted: 244,
    text: 252,
    selected: 255,
    border: 239,
    scan: 252,
};

impl Default for Palette {
    fn default() -> Self {
        STOCK
    }
}

/// The palette in force, set once before anything is drawn.
static IN_FORCE: OnceLock<Palette> = OnceLock::new();

/// Use `palette` for the life of the process.
///
/// Only the first call counts. A second one is a second opinion arriving after the screen has
/// already been painted with the first, which is worse than being ignored.
pub fn adopt(palette: Palette) {
    let _ = IN_FORCE.set(palette);
}

/// The palette in force.
#[must_use]
pub fn palette() -> &'static Palette {
    IN_FORCE.get_or_init(Palette::default)
}

/// A palette index.
const fn at(index: u8) -> Color {
    Color::Indexed(index)
}

/// Spinners, list cursors, markdown bullets, inline code.
#[must_use]
pub fn accent() -> Color {
    at(palette().accent)
}
/// Success states, added diff lines, a tool that finished.
#[must_use]
pub fn success() -> Color {
    at(palette().success)
}
/// Warnings and elevated context usage.
#[must_use]
pub fn warning() -> Color {
    at(palette().warning)
}
/// Errors, removed diff lines, a tool that failed.
#[must_use]
pub fn error() -> Color {
    at(palette().error)
}
/// Markdown headings.
#[must_use]
pub fn heading() -> Color {
    at(palette().heading)
}
/// Fenced code blocks.
#[must_use]
pub fn code_block() -> Color {
    at(palette().code)
}
/// The characters you have already typed, wherever they appear in a candidate.
#[must_use]
pub fn match_() -> Color {
    at(palette().match_)
}
/// Behind a tool block and behind a menu.
#[must_use]
pub fn block_bg() -> Color {
    at(palette().block_bg)
}
/// Behind a user message, and behind the row of a list you are on.
#[must_use]
pub fn raised_bg() -> Color {
    at(palette().raised_bg)
}
/// The rule above and below the prompt.
#[must_use]
pub fn rule() -> Color {
    at(palette().rule)
}
/// Tertiary text; the footer lives here.
#[must_use]
pub fn dim() -> Color {
    at(palette().dim)
}
/// Secondary text: tool output, quotes, reasoning, unchanged diff lines.
#[must_use]
pub fn muted() -> Color {
    at(palette().muted)
}
/// Default foreground.
#[must_use]
pub fn text() -> Color {
    at(palette().text)
}
/// The row of a list you are on.
#[must_use]
pub fn selected() -> Color {
    at(palette().selected)
}
/// The prompt's border at rest.
#[must_use]
pub fn border() -> Color {
    at(palette().border)
}

/// `amount` of the way from the resting border to the brightest point of the scan.
///
/// A step along the run of indices between the two rather than a blend of them, for the reason
/// given on [`Palette::scan`]. A palette whose scan is not above its border has no run to walk,
/// and gets the border rather than an inverted gradient.
#[must_use]
pub fn scan(amount: f32) -> Color {
    let Palette { border, scan, .. } = *palette();
    let Some(span) = scan.checked_sub(border) else {
        return at(border);
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the run first"
    )]
    let step = (f32::from(span) * amount.clamp(0.0, 1.0)).round() as u8;
    at(border + step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_lit_is_the_resting_border() {
        assert_eq!(scan(0.0), border());
    }

    #[test]
    fn fully_lit_is_the_top_of_the_run() {
        assert_eq!(scan(1.0), at(palette().scan));
    }

    #[test]
    fn the_scan_only_climbs() {
        let seen: Vec<Color> = (0..=10u8).map(|n| scan(f32::from(n) / 10.0)).collect();
        for pair in seen.windows(2) {
            let (Color::Indexed(a), Color::Indexed(b)) = (pair[0], pair[1]) else {
                panic!("the run is indexed");
            };
            assert!(a <= b, "it only climbs: {a} then {b}");
        }
    }

    #[test]
    fn an_amount_off_the_end_stays_on_the_run() {
        assert_eq!(scan(-1.0), border());
        assert_eq!(scan(9.0), at(palette().scan));
    }

    #[test]
    fn a_scan_below_its_border_is_no_gradient_rather_than_a_backwards_one() {
        let upside_down = Palette {
            border: 250,
            scan: 240,
            ..STOCK
        };
        let Some(_) = upside_down.scan.checked_sub(upside_down.border) else {
            return;
        };
        panic!("this palette has a run after all; the guard is testing nothing");
    }

    #[test]
    fn nothing_that_sits_on_the_screen_is_lost_in_it() {
        // The block backgrounds and the border have to read as *on* the screen. Two steps off
        // black is a lift; below that is a hole, and the first pass at this put tool blocks and
        // menus underneath the terminal's own background.
        let p = STOCK;
        assert!(p.block_bg > 232, "a block is not black");
        assert!(p.raised_bg > p.block_bg, "a raised row beats the block");
        assert!(p.border > 232, "the border is not black");
    }

    #[test]
    fn the_stock_palette_reads_as_an_ordinary_terminal() {
        // Somebody who has configured nothing gets the ordinary meanings, because that is what
        // their palette actually holds.
        assert_eq!(STOCK.error, 1, "red");
        assert_eq!(STOCK.success, 2, "green");
        assert_eq!(STOCK.warning, 3, "yellow");
        assert_eq!(STOCK.accent, 6, "cyan");
    }

    #[test]
    fn text_weights_are_ordered() {
        let p = STOCK;
        assert!(p.dim < p.muted, "the footer is quieter than tool output");
        assert!(p.muted < p.text, "tool output is quieter than prose");
        assert!(p.text < p.selected, "and a selected row beats prose");
    }
}
