//! The colours, which are the terminal's own: every one is an index into the palette the terminal
//! already has. The defaults assume nothing but a dark screen — the bright half of each xterm pair
//! and the top fifth of the 232-255 greyscale, the half meant to be read off a dark background.
//! Every one is settable by name from `magi.ui` in Lua; roles sharing a default stay separate names.
//!
//! ```lua
//! magi.ui.accent = 1
//! magi.ui.muted  = 8
//! ```

use ratatui::style::Color;
use std::sync::OnceLock;

/// Declare the palette once — struct, defaults, accessors and the names a config may set — from one list.
macro_rules! palette {
    ($($name:ident = $default:literal, $doc:literal;)*) => {
        /// Every colour the UI draws with, as palette indices.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Palette {
            $(#[doc = $doc] pub $name: u8,)*
        }

        pub const STOCK: Palette = Palette { $($name: $default,)* };

        impl Palette {
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            /// Take whatever `given` answers for; a name it has no answer for keeps its default.
            pub fn overlay(&mut self, given: &dyn Fn(&str) -> Option<u8>) {
                $(if let Some(value) = given(stringify!($name)) { self.$name = value; })*
            }
        }

        $(#[doc = $doc] #[must_use] pub fn $name() -> Color { at(palette().$name) })*
    };
}

// Secondary text lives in the 246-251 band and surfaces sit above 236: below that a menu row is
// barely above the screen. The hues are the bright six, 9/10/11 rather than 1/2/3.
palette! {
    accent = 14, "Spinners, list cursors, markdown bullets.";
    success = 10, "Success states.";
    warning = 11, "Warnings and elevated context usage.";
    error = 9, "Errors, and a tool that failed.";
    typed = 13, "The characters you have already typed, wherever they appear in a candidate.";

    md_heading = 11, "Markdown headings.";
    md_code = 14, "Inline code spans.";
    md_code_block = 10, "Fenced code block contents.";
    md_quote = 250, "Block quote text and its rule.";

    diff_added = 40, "Added lines in a diff.";
    diff_removed = 167, "Removed lines in a diff.";
    diff_marker = 214, "A diff's file and hunk headers, which are neither added nor removed.";
    diff_context = 245, "Unchanged context lines in a diff.";

    tool_bg = 237, "Behind a tool block.";
    tool_title = 255, "The tool's name, when it is still running.";
    tool_ok = 10, "The tool's name, when it finished.";
    tool_failed = 9, "The tool's name, when it failed.";
    tool_output = 251, "A tool's output.";
    tool_fold = 246, "The note saying how much of a result is not shown.";
    tool_seam = 235, "The rule between what a call was asked and what it answered. A line rather than a surface, so it may sit below the floor a fill has to keep.";
    block_frame = 237, "A transcript block's own frame. Not the prompt's border: a box in a scrolling record should sit further back than the thing you are typing into.";

    menu_selected_bg = 241, "Behind the row you are on.";
    menu_selected = 255, "The row you are on.";
    menu_detail = 250, "What a row says about itself, beside its name.";
    menu_detail_selected = 255, "The same, on the selected row.";
    menu_meta = 247, "Counts and scroll markers on the heading.";

    // Not brightened with the rest: border and scan are two ends of one gradient, and the further
    // apart they sit the more of a comet there is to see.
    border = 240, "The prompt's border with nothing lit, and the floor of its scan.";
    scan = 255, "The brightest point of the light travelling along the border.";
    hint = 241, "The empty prompt's placeholder. Well under the text, so it reads as a label rather than as something you wrote.";
    rule = 245, "The rule above and below a quotation.";

    message_bg = 237, "Behind something you said.";
    message_text = 255, "Something you said.";
    // The tag on a message block is a hue, not a grey: a tool block wears a reversed chip in white,
    // green or red, and a white tag on a background three steps from it was indistinguishable.
    said_by_you = 13, "The `USER` tag on something you said.";
    said_by_agent = 14, "The tag on a message from another instance.";
    thinking = 249, "Reasoning blocks.";
    text = 253, "Default foreground.";
    muted = 250, "Secondary text.";
    dim = 246, "Tertiary text; the footer lives here.";
}

impl Default for Palette {
    fn default() -> Self {
        STOCK
    }
}

/// The palette in force, set once before anything is drawn.
static IN_FORCE: OnceLock<Palette> = OnceLock::new();

/// Use `palette` for the life of the process. Only the first call counts.
pub fn adopt(palette: Palette) {
    let _ = IN_FORCE.set(palette);
}

#[must_use]
pub fn palette() -> &'static Palette {
    IN_FORCE.get_or_init(Palette::default)
}

const fn at(index: u8) -> Color {
    Color::Indexed(index)
}

/// `amount` of the way from the resting border to the brightest point of the scan: a step along the
/// run of indices between them rather than a blend, since nothing here knows what the colours are.
/// A palette whose scan is not above its border has no run to walk, and gets the border.
#[must_use]
pub fn scan_at(amount: f32) -> Color {
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
        assert_eq!(scan_at(0.0), border());
    }

    #[test]
    fn fully_lit_is_the_top_of_the_run() {
        assert_eq!(scan_at(1.0), at(palette().scan));
    }

    #[test]
    fn the_scan_only_climbs() {
        let seen: Vec<Color> = (0..=10u8).map(|n| scan_at(f32::from(n) / 10.0)).collect();
        for pair in seen.windows(2) {
            let (Color::Indexed(a), Color::Indexed(b)) = (pair[0], pair[1]) else {
                panic!("the run is indexed");
            };
            assert!(a <= b, "it only climbs: {a} then {b}");
        }
    }

    #[test]
    fn an_amount_off_the_end_stays_on_the_run() {
        assert_eq!(scan_at(-1.0), border());
        assert_eq!(scan_at(9.0), at(palette().scan));
    }

    #[test]
    fn an_overlay_takes_what_it_is_given_and_nothing_else() {
        let mut chosen = STOCK;
        chosen.overlay(&|name| (name == "accent").then_some(1));
        assert_eq!(chosen.accent, 1);
        assert_eq!(chosen.muted, STOCK.muted, "and left the rest alone");
    }

    #[test]
    fn every_field_can_be_named() {
        // The macro builds struct, defaults, accessors and this list from one place. Counted rather
        // than compared because there is nothing else to compare it against.
        let mut all = STOCK;
        all.overlay(&|_| Some(200));
        assert_eq!(all.accent, 200);
        assert_eq!(all.scan, 200);
        assert!(Palette::NAMES.len() > 25, "{}", Palette::NAMES.len());
        assert!(Palette::NAMES.contains(&"tool_output"));
    }

    #[test]
    fn nothing_that_sits_on_the_screen_is_lost_in_it() {
        // 232-236 is the bottom fifth of the greyscale: a block painted there is a hole.
        for surface in [
            STOCK.tool_bg,
            STOCK.menu_selected_bg,
            STOCK.message_bg,
            STOCK.border,
        ] {
            assert!(surface > 236, "{surface} is as good as black");
        }
    }

    #[test]
    fn no_secondary_text_is_left_in_the_dark_half() {
        // Everything a person actually reads sits in the top fifth of the greyscale.
        for weight in [
            STOCK.dim,
            STOCK.muted,
            STOCK.text,
            STOCK.menu_detail,
            STOCK.menu_meta,
            STOCK.tool_output,
            STOCK.tool_fold,
            STOCK.md_quote,
            STOCK.thinking,
        ] {
            assert!(weight >= 246, "{weight} is too dark to read comfortably");
        }
        // `hint` is deliberately not in that list: a placeholder as bright as what you type reads
        // as something already in the box.
        let hint = STOCK.hint;
        assert!(hint < 246, "the placeholder is as loud as the text: {hint}");
        assert!(hint > 236, "and not a hole in the screen: {hint}");
    }

    #[test]
    fn the_scan_has_a_run_long_enough_to_read_as_one() {
        // Twelve steps is the floor at which a comet reads as a comet rather than two greys.
        let run = STOCK.scan.saturating_sub(STOCK.border);
        assert!(
            run >= 12,
            "only {run} steps between the border and the scan"
        );
    }

    #[test]
    fn the_stock_palette_reads_as_an_ordinary_terminal() {
        // The bright half of each pair: 1, 2 and 3 are the dark ones on most palettes.
        assert_eq!(STOCK.error, 9, "bright red");
        assert_eq!(STOCK.success, 10, "bright green");
        assert_eq!(STOCK.warning, 11, "bright yellow");
        assert_eq!(STOCK.accent, 14, "bright cyan");
    }

    #[test]
    #[expect(
        clippy::assertions_on_constants,
        reason = "the constants are the subject"
    )]
    fn text_weights_are_ordered() {
        assert!(STOCK.dim < STOCK.muted, "the footer is quieter than output");
        assert!(STOCK.muted < STOCK.text, "output is quieter than prose");
        assert!(
            STOCK.text < STOCK.menu_selected,
            "a selected row beats prose"
        );
    }
}
