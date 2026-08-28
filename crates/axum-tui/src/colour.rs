//! The colours, which are the terminal's own.
//!
//! There is no theme. There was one — Pi's `dark.json`, ported hex by hex — and every value in it
//! was a decision about what somebody's terminal should look like, made by somebody who has never
//! seen it. A person who has set their terminal's palette has already answered that question, and
//! answering it again over the top is how a program ends up being the one window on the screen
//! that does not match the others.
//!
//! So everything here is an **index into the palette the terminal already has**, and what those
//! indices actually look like is not this program's business.
//!
//! **0-15, the sixteen.** Slot 1 is the accent. That is not the ANSI convention — conventionally
//! 1 is red — it is [lule](https://github.com/bresilla/lule)'s, which generates a palette from a
//! wallpaper and aliases both `accent` and `cursor` to slot 1. Its slots 1-6 are the six most
//! chromatic pigments of an image in chroma order, so they carry no hue meaning at all and there
//! is nothing to be faithful to. The one role that wants to be unmistakable, `ERROR`, sits in the
//! bright six instead, where a stock palette also happens to put red.
//!
//! **232-255, the ramp.** On a plain xterm palette this is the 24-step greyscale. Under lule it is
//! better than that: `black → colour 0 → accent → colour 15 → white`, so the middle of it is the
//! accent at every lightness. Every neutral in this file is a position on that ramp, and so is the
//! prompt's scan — which means the scan is a gradient through the person's own accent colour
//! without this file knowing what colour that is.

use ratatui::style::Color;

/// A palette index.
const fn at(index: u8) -> Color {
    Color::Indexed(index)
}

// ------------------------------------------------------------------ the six
//
// Hues, for the few things that are a *kind* rather than a weight.

/// Spinners, list cursors, markdown bullets, inline code, the scan's brightest point.
pub const ACCENT: Color = at(1);
/// Success states and added diff lines.
pub const SUCCESS: Color = at(2);
/// Warnings and elevated context usage.
pub const WARNING: Color = at(3);
/// Markdown headings.
pub const HEADING: Color = at(5);
/// Fenced code blocks.
pub const CODE_BLOCK: Color = at(6);
/// Errors, removed diff lines, and a tool that failed.
///
/// The bright six rather than slot 4, because this is the one role that has to be unmistakable
/// and a stock palette puts red here.
pub const ERROR: Color = at(9);
/// The characters you have already typed, wherever they appear in a candidate.
///
/// The accent, louder — the one thing that makes a long list scannable.
pub const MATCH: Color = at(14);

// ------------------------------------------------------------------ the ramp
//
// Weights. Low is dark, high is light, and the middle is tinted with the accent.

/// Behind a tool block and behind a menu, so each reads as one object.
pub const BLOCK_BG: Color = at(233);
/// Behind a user message, and behind the row of a list you are on.
pub const RAISED_BG: Color = at(236);
/// The rule above and below the prompt.
pub const RULE: Color = at(236);
/// The prompt's border at rest.
pub const BORDER: Color = at(237);
/// Tertiary text; the footer lives here.
pub const DIM: Color = at(243);
/// Secondary text: tool output, quotes, reasoning, unchanged diff lines.
pub const MUTED: Color = at(247);
/// Default foreground.
pub const TEXT: Color = at(252);
/// The row of a list you are on, which has to beat [`TEXT`] to read as selected.
pub const SELECTED: Color = at(255);

// -------------------------------------------------------------- the scan
//
// The prompt's border is a gradient rather than two colours, so it gets the ramp by the step
// rather than by the name. Under lule these sixteen walk `colour 0 → accent → colour 15`: a comet
// running them goes dark, through the accent, to bright.

/// Where the border sits with nothing lit, and where the brightest point of the scan sits.
const SCAN_FLOOR: u8 = 237;
const SCAN_PEAK: u8 = 252;

/// `amount` of the way from the resting border to the brightest point of the scan.
///
/// A step on the ramp rather than a blend of two colours. Blending needs to know what the colours
/// *are*, which is exactly what this file has given up knowing — and the ramp is already a
/// gradient, so there is nothing to compute.
#[must_use]
pub fn scan(amount: f32) -> Color {
    let span = f32::from(SCAN_PEAK - SCAN_FLOOR);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the ramp first"
    )]
    let step = (span * amount.clamp(0.0, 1.0)).round() as u8;
    at(SCAN_FLOOR + step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_lit_is_the_resting_border() {
        assert_eq!(scan(0.0), BORDER);
    }

    #[test]
    fn fully_lit_is_the_top_of_the_ramp() {
        assert_eq!(scan(1.0), at(SCAN_PEAK));
    }

    #[test]
    fn the_scan_climbs_without_skipping_the_middle() {
        // The middle is the point: under lule these steps walk through the accent, so a comet
        // passing along them is coloured without this file naming a colour.
        let seen: Vec<Color> = (0..=10u8).map(|n| scan(f32::from(n) / 10.0)).collect();
        assert!(
            seen.iter()
                .any(|c| matches!(c, Color::Indexed(n) if (242..=247).contains(n))),
            "the middle of the ramp is used"
        );
        for pair in seen.windows(2) {
            let (Color::Indexed(a), Color::Indexed(b)) = (pair[0], pair[1]) else {
                panic!("the ramp is indexed");
            };
            assert!(a <= b, "it only climbs: {a} then {b}");
        }
    }

    #[test]
    fn an_amount_off_the_end_stays_on_the_ramp() {
        assert_eq!(scan(-1.0), BORDER);
        assert_eq!(scan(9.0), at(SCAN_PEAK));
    }

    #[test]
    fn every_colour_is_the_terminals_own() {
        // The whole point: no `Color::Rgb` anywhere, so a palette the person set is the palette
        // they get.
        for colour in [
            ACCENT, SUCCESS, WARNING, HEADING, CODE_BLOCK, ERROR, MATCH, BLOCK_BG, RAISED_BG, RULE,
            BORDER, DIM, MUTED, TEXT, SELECTED,
        ] {
            assert!(
                matches!(colour, Color::Indexed(_)),
                "{colour:?} is not a palette index"
            );
        }
    }
}
