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
//! **The defaults assume nothing but a dark screen.** They are the ordinary xterm reading — 9 is
//! bright red, 10 bright green, 14 bright cyan, and 232-255 the 24-step greyscale — taken from the
//! bright half of each pair and the top fifth of that greyscale, because that is the half meant to
//! be read *off* a dark background. The first pass took the dark half of both and the result was a
//! UI you squint at.
//!
//! **Every one of them is settable**, by the name in the table below, from `axon.ui` in Lua. Roles
//! that share a default are still separate names: `tool_output` and `md_quote` happen to be the
//! same grey and are not the same decision, and a config that wants to move one should not have to
//! move the other.
//!
//! ```lua
//! axon.ui.accent = 1
//! axon.ui.muted  = 8
//! ```

use ratatui::style::Color;
use std::sync::OnceLock;

/// Declare the palette once: the struct, the defaults, the accessors, and the names a config may
/// set, all from one list so none of them can fall out of step with the others.
macro_rules! palette {
    ($($name:ident = $default:literal, $doc:literal;)*) => {
        /// Every colour the UI draws with, as palette indices.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Palette {
            $(#[doc = $doc] pub $name: u8,)*
        }

        /// What a terminal nobody has configured looks like.
        pub const STOCK: Palette = Palette { $($name: $default,)* };

        impl Palette {
            /// Every name `axon.ui` recognises as a colour.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            /// Take whatever `given` answers for, and keep the rest.
            ///
            /// A name it has no answer for is left at the default rather than zeroed: a config
            /// that sets three colours is setting three colours.
            pub fn overlay(&mut self, given: &dyn Fn(&str) -> Option<u8>) {
                $(if let Some(value) = given(stringify!($name)) { self.$name = value; })*
            }
        }

        $(#[doc = $doc] #[must_use] pub fn $name() -> Color { at(palette().$name) })*
    };
}

// The greyscale runs 232 (#080808) to 255 (#eeeeee), and the first pass sat far too low in it:
// text at 252 but everything beside it at 241 and 244, on backgrounds of 234. Those are 38%,
// 50% and 11% grey — a menu whose rows were barely above the screen and whose detail column was
// half lit. Secondary text lives in the 246-251 band now and surfaces sit above 236, which is the
// difference between quiet and unreadable.
//
// The hues moved to the bright six for the same reason. On most palettes 1, 2 and 3 are the dark
// half of the pair and 9, 10 and 11 are the one meant to be read off a dark background.
palette! {
    // ---------------------------------------------------------------- hues
    accent = 14, "Spinners, list cursors, markdown bullets.";
    success = 10, "Success states.";
    warning = 11, "Warnings and elevated context usage.";
    error = 9, "Errors, and a tool that failed.";
    typed = 13, "The characters you have already typed, wherever they appear in a candidate.";

    // ------------------------------------------------------------ markdown
    md_heading = 11, "Markdown headings.";
    md_code = 14, "Inline code spans.";
    md_code_block = 10, "Fenced code block contents.";
    md_quote = 250, "Block quote text and its rule.";

    // ---------------------------------------------------------------- diffs
    diff_added = 40, "Added lines in a diff.";
    diff_removed = 167, "Removed lines in a diff.";
    diff_marker = 214, "A diff's file and hunk headers, which are neither added nor removed.";
    diff_context = 245, "Unchanged context lines in a diff.";

    // ---------------------------------------------------------------- tools
    tool_bg = 237, "Behind a tool block.";
    tool_title = 255, "The tool's name, when it is still running.";
    tool_ok = 10, "The tool's name, when it finished.";
    tool_failed = 9, "The tool's name, when it failed.";
    tool_output = 251, "A tool's output.";
    tool_fold = 246, "The note saying how much of a result is not shown.";

    // ---------------------------------------------------------------- menus
    menu_selected_bg = 241, "Behind the row you are on.";
    menu_selected = 255, "The row you are on.";
    menu_detail = 250, "What a row says about itself, beside its name.";
    menu_detail_selected = 255, "The same, on the selected row.";
    menu_meta = 247, "Counts and scroll markers on the heading.";

    // -------------------------------------------------------------- the box
    //
    // The one thing that is *not* brightened with the rest. A border is not text, it is what the
    // light moves against, and the two are one gradient: the further apart they sit the more of
    // a comet there is to see. Raised to 245 alongside everything else, the run was ten steps
    // from an already-bright frame and the scan vanished into its own border.
    border = 240, "The prompt's border with nothing lit, and the floor of its scan.";
    scan = 255, "The brightest point of the light travelling along the border.";
    hint = 241, "The empty prompt's placeholder. Well under the text, so it reads as a label rather than as something you wrote.";
    rule = 245, "The rule above and below a quotation.";

    // ------------------------------------------------------------ the rest
    message_bg = 237, "Behind something you said.";
    message_text = 255, "Something you said.";
    // The tag on a message block, and why it is not one of the greys.
    //
    // A tool block wears a reversed chip too, and its colours are the outcome's: white while it
    // runs, green when it finished, red when it failed. A message tag in white on a background
    // three steps from the tool block's own made the two indistinguishable at a glance — and a
    // tool block is the one that folds, so half the screen looked like it had a handle on it.
    // These two are hues no tool state uses.
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

/// `amount` of the way from the resting border to the brightest point of the scan.
///
/// A step along the run of indices between the two rather than a blend of them. Blending needs to
/// know what the colours *are*, which is exactly what this design has given up knowing, and a
/// contiguous run of palette indices is already a gradient — under a generated palette it may even
/// be a gradient through the person's own accent. A palette whose scan is not above its border has
/// no run to walk, and gets the border rather than an inverted one.
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
        // The macro builds the struct, the defaults, the accessors and this list from one place,
        // so a colour that exists is a colour a config can set. Counted rather than compared
        // because there is nothing else to compare it against.
        let mut all = STOCK;
        all.overlay(&|_| Some(200));
        assert_eq!(all.accent, 200);
        assert_eq!(all.scan, 200);
        assert!(Palette::NAMES.len() > 25, "{}", Palette::NAMES.len());
        assert!(Palette::NAMES.contains(&"tool_output"));
    }

    #[test]
    fn nothing_that_sits_on_the_screen_is_lost_in_it() {
        // A surface has to read as *on* the screen, and read as a surface rather than a shadow.
        // 232-236 is the bottom fifth of the greyscale: a block painted there is a hole on a
        // dark terminal, which is what the first pass at this drew.
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
        // The complaint this answers: text at 241 and 244 on a 234 background is a menu you
        // squint at. Everything a person actually reads sits in the top fifth of the greyscale.
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
        // `hint` is deliberately not in that list. Every other weight here is text somebody
        // reads; the placeholder is a label they are meant to look past, and one as bright as
        // what they type reads as something already in the box.
        let hint = STOCK.hint;
        assert!(hint < 246, "the placeholder is as loud as the text: {hint}");
        assert!(hint > 236, "and not a hole in the screen: {hint}");
    }

    #[test]
    fn the_scan_has_a_run_long_enough_to_read_as_one() {
        // The border and the scan are two ends of a gradient, so a border brightened towards the
        // scan is a scan nobody can see. Twelve steps is the floor at which a comet still reads
        // as a comet rather than as two slightly different greys.
        let run = STOCK.scan.saturating_sub(STOCK.border);
        assert!(
            run >= 12,
            "only {run} steps between the border and the scan"
        );
    }

    #[test]
    fn the_stock_palette_reads_as_an_ordinary_terminal() {
        // The bright half of each pair: on most palettes 1, 2 and 3 are the dark ones and 9, 10
        // and 11 are the ones meant to be read off a dark background.
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
