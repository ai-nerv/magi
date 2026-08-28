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
//! **232-255, the ramp — and what it is not for.** On a plain xterm palette this is the 24-step
//! greyscale. Under lule it is `black → colour 0 → accent → colour 15 → white`, which makes only
//! its two *ends* neutral: 232-236 walks black to the background and 251-255 walks the foreground
//! to white, while everything between is the accent at some lightness. Index 244 is not a grey, it
//! is colour 1.
//!
//! That is one thing and one thing only: **the prompt's scan**, which wants exactly that — a comet
//! running the ramp goes dark, through the person's own accent, to bright, without this file
//! knowing what colour that is. Every *neutral* comes from the grey slots instead. Reading a middle
//! index as "a grey" put the footer, tool output, quotations, list details and the placeholder all
//! in the accent, and the answer to "why is everything colour 1" was that it was.

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

// ------------------------------------------------------------------ the greys
//
// Weights. Text neutrals come from the two grey slots and the foreground, never from the middle
// of the ramp — which is not grey. Backgrounds come from the ramp's dark end, which is, because
// 232-236 is black walking to the terminal's own background and picks up no tint on the way.

/// Behind a tool block and behind a menu, so each reads as one object.
pub const BLOCK_BG: Color = at(233);
/// Behind a user message, and behind the row of a list you are on.
pub const RAISED_BG: Color = at(235);
/// The rule above and below the prompt.
pub const RULE: Color = at(8);
/// Tertiary text; the footer lives here.
///
/// Slot 7 rather than 8. Conventionally 8 is the dimmer of the pair — it is "bright black" — but
/// lule builds the two greys from 100 and 170 and hands them out in that order for a dark theme,
/// so under the palette this is written for 7 is the darker one. On a stock palette the two swap
/// and the footer comes out a shade brighter than tool output, which is a shade, not a bug.
pub const DIM: Color = at(7);
/// Secondary text: tool output, quotes, reasoning, unchanged diff lines.
pub const MUTED: Color = at(8);
/// Default foreground.
///
/// Slot 15, so what the agent says is drawn in the colour everything else in the terminal says
/// things in.
pub const TEXT: Color = at(15);
/// The row of a list you are on, which has to beat [`TEXT`] to read as selected.
///
/// The top of the ramp, which is white under any palette — 232-255 ends `colour 15 → white`, and
/// a stock greyscale ends there too.
pub const SELECTED: Color = at(255);

// -------------------------------------------------------------- the scan
//
// The one place the middle of the ramp is wanted. The prompt's border is a gradient rather than
// two colours, so it gets the ramp by the step rather than by the name: under lule these fifteen
// walk `colour 0 → accent → colour 15`, and a comet running them goes dark, through the accent,
// to bright.

/// Where the border sits with nothing lit, and where the brightest point of the scan sits.
const SCAN_FLOOR: u8 = 237;
const SCAN_PEAK: u8 = 252;

/// The prompt's border at rest.
///
/// The floor of the scan, and defined from it so the two cannot drift apart: a border a shade off
/// the unlit end of its own gradient shows as a seam wherever no head is.
pub const BORDER: Color = at(SCAN_FLOOR);

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

    #[test]
    fn no_neutral_is_taken_from_the_middle_of_the_ramp() {
        // 237-250 is not grey — under lule it walks the accent — so a neutral read from there is
        // the accent wearing a neutral's name. That is how the footer, tool output, quotations,
        // list details and the placeholder all came out colour 1.
        for colour in [BLOCK_BG, RAISED_BG, RULE, DIM, MUTED, TEXT, SELECTED] {
            let Color::Indexed(n) = colour else {
                panic!("{colour:?} is not a palette index");
            };
            assert!(
                !(237..=250).contains(&n),
                "{n} is in the tinted middle of the ramp"
            );
        }
    }
}
