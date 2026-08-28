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
//! **Every one of them is settable**, by the name in the table below, from `axum.ui` in Lua. Roles
//! that share a default are still separate names: `tool_output` and `md_quote` happen to be the
//! same grey and are not the same decision, and a config that wants to move one should not have to
//! move the other.
//!
//! ```lua
//! axum.ui.accent = 1
//! axum.ui.muted  = 8
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
            /// Every name `axum.ui` recognises as a colour.
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

palette! {
    // ---------------------------------------------------------------- hues
    accent = 6, "Spinners, list cursors, markdown bullets.";
    success = 2, "Success states.";
    warning = 3, "Warnings and elevated context usage.";
    error = 1, "Errors, and a tool that failed.";
    typed = 14, "The characters you have already typed, wherever they appear in a candidate.";

    // ------------------------------------------------------------ markdown
    md_heading = 11, "Markdown headings.";
    md_code = 6, "Inline code spans.";
    md_code_block = 2, "Fenced code block contents.";
    md_quote = 244, "Block quote text and its rule.";

    // ---------------------------------------------------------------- diffs
    diff_added = 2, "Added lines in a diff.";
    diff_removed = 1, "Removed lines in a diff.";
    diff_context = 244, "Unchanged context lines in a diff.";

    // ---------------------------------------------------------------- tools
    tool_bg = 234, "Behind a tool block.";
    tool_title = 252, "The tool's name, when it is still running.";
    tool_ok = 2, "The tool's name, when it finished.";
    tool_failed = 1, "The tool's name, when it failed.";
    tool_output = 244, "A tool's output.";
    tool_fold = 241, "The note saying how much of a result is not shown.";

    // ---------------------------------------------------------------- menus
    menu_bg = 234, "Behind every row of a list, so it reads as one object.";
    menu_selected_bg = 237, "Behind the row you are on.";
    menu_selected = 255, "The row you are on.";
    menu_detail = 244, "What a row says about itself, beside its name.";
    menu_detail_selected = 252, "The same, on the selected row.";
    menu_meta = 241, "Counts and scroll markers on the heading.";

    // -------------------------------------------------------------- the box
    border = 239, "The prompt's border with nothing lit, and the floor of its scan.";
    scan = 252, "The brightest point of the light travelling along the border.";
    hint = 241, "The prompt's own text, before you type anything.";
    rule = 239, "The rule above and below a quotation.";

    // ------------------------------------------------------------ the rest
    message_bg = 237, "Behind something you said.";
    message_text = 252, "Something you said.";
    thinking = 244, "Reasoning blocks.";
    text = 252, "Default foreground.";
    muted = 244, "Secondary text.";
    dim = 241, "Tertiary text; the footer lives here.";
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
    #[expect(
        clippy::assertions_on_constants,
        reason = "the constants are the subject"
    )]
    fn nothing_that_sits_on_the_screen_is_lost_in_it() {
        // A surface has to read as *on* the screen. Two steps off black is a lift; below that is
        // a hole, and the first pass at this put tool blocks and menus underneath the terminal's
        // own background.
        for surface in [STOCK.tool_bg, STOCK.menu_bg, STOCK.message_bg, STOCK.border] {
            assert!(surface > 232, "{surface} is as good as black");
        }
        assert!(
            STOCK.menu_selected_bg > STOCK.menu_bg,
            "a row beats its list"
        );
    }

    #[test]
    fn the_stock_palette_reads_as_an_ordinary_terminal() {
        assert_eq!(STOCK.error, 1, "red");
        assert_eq!(STOCK.success, 2, "green");
        assert_eq!(STOCK.warning, 3, "yellow");
        assert_eq!(STOCK.accent, 6, "cyan");
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
