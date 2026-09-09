//! The characters the UI is drawn out of, every one settable by name from `magi.ui` in Lua. Width
//! is the caller's problem: a glyph two columns wide draws two columns wide, and nothing measures.
//!
//! ```lua
//! magi.ui.corner_top_left = "┌"
//! magi.ui.marker          = "▶ "
//! magi.ui.spinner         = { "◐", "◓", "◑", "◒" }
//! ```

use std::sync::OnceLock;

/// Declare the glyphs once — struct, defaults, accessors and the settable names — from one list.
macro_rules! glyphs {
    ($($name:ident = $default:literal, $doc:literal;)*) => {
        /// Every character the UI is drawn out of.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Glyphs {
            $(#[doc = $doc] pub $name: String,)*
            /// The spinner, a frame at a time. An empty list is refused on the way in, because a
            /// spinner with no frames is a division by zero while somebody is watching.
            pub spinner: Vec<String>,
            /// What the empty prompt writes to itself once left alone. `a ~~b~~ c` means "write
            /// `a b`, take the `b` back, write `c`", a character at a time. See [`crate::tease`].
            pub placeholders: Vec<String>,
            /// What the empty prompt says before any of that. Plain, and nothing to read twice.
            pub openers: Vec<String>,
        }

        impl Default for Glyphs {
            fn default() -> Self {
                Self {
                    $($name: $default.to_owned(),)*
                    spinner: SPINNER.iter().map(|f| (*f).to_owned()).collect(),
                    placeholders: PLACEHOLDERS.iter().map(|p| (*p).to_owned()).collect(),
                    openers: OPENERS.iter().map(|o| (*o).to_owned()).collect(),
                }
            }
        }

        impl Glyphs {
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            pub fn overlay(&mut self, given: &dyn Fn(&str) -> Option<String>) {
                $(if let Some(value) = given(stringify!($name)) { self.$name = value; })*
            }
        }

        $(#[doc = $doc] #[must_use] pub fn $name() -> &'static str { &glyphs().$name })*
    };
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

glyphs! {
    corner_top_left = "╭", "The prompt box's top-left corner.";
    corner_top_right = "╮", "The prompt box's top-right corner.";
    corner_bottom_left = "╰", "The prompt box's bottom-left corner.";
    corner_bottom_right = "╯", "The prompt box's bottom-right corner.";
    edge_horizontal = "─", "The prompt box's top and bottom edges.";
    edge_vertical = "│", "The prompt box's sides.";
    divider_left = "├", "Where the rule between the prompt and its menu meets the left side.";
    divider_right = "┤", "The same, on the right.";
    marker = "❯ ", "In front of the row of a list you are on.";
    no_marker = "  ", "In front of every other row, so the names stay in one column.";
    ellipsis = "…", "Where something was cut to fit.";
    bullet = "• ", "A markdown list item.";
    more_rule = "─ ", "Repeated along an edge the transcript continues past.";
    block_top_left = "┌", "Where a transcript block's top edge starts.";
    block_top_right = "┐", "Where it ends.";
    block_bottom_left = "└", "Where its bottom edge starts.";
    block_bottom_right = "┘", "Where it ends.";
    block_edge = "─", "Repeated along a transcript block's top and bottom edges.";
    expand = "▸", "On the top edge of a folded tool block: click to open it.";
    collapse = "▾", "On the top edge of an open tool block: click to fold it.";
    copy = "⧉", "On the top edge of a block: click to put what it says on the clipboard.";
    running = "·", "Beside a call that has been made and has not come back.";
    outcome_ok = "✓", "Beside a call that came back without an error.";
    outcome_failed = "✗", "Beside a call that reported a problem.";
    quote_rule = "│ ", "Down the left of a block quote.";
    notice_rule = "│ ", "Down the left of something the UI itself is saying.";
    placeholder = "ask anything, or : for commands", "The prompt, before you type anything.";
    placeholder_short = ": for commands", "The same, on a screen too narrow for the whole of it.";
    no_model = "no-model", "What the footer says when nothing is configured to answer.";
    decrypt_pool = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789#%&@$?*+=-<>/", "The characters the opening scramble draws from.";
    flicker_pool = "#%&@$?*+=~^<>/", "The symbols a character glitches to and back from.";
    type_stages = "·*#", "What a character you have just typed passes through, in order.";
}

/// The glyphs in force, set once before anything is drawn.
static IN_FORCE: OnceLock<Glyphs> = OnceLock::new();

/// Use `glyphs` for the life of the process. Only the first call counts.
pub fn adopt(glyphs: Glyphs) {
    let _ = IN_FORCE.set(glyphs);
}

#[must_use]
pub fn glyphs() -> &'static Glyphs {
    IN_FORCE.get_or_init(Glyphs::default)
}

#[must_use]
pub fn spinner(tick: usize) -> &'static str {
    let frames = &glyphs().spinner;
    if frames.is_empty() {
        return "";
    }
    &frames[tick % frames.len()]
}

#[must_use]
pub fn placeholders() -> &'static [String] {
    &glyphs().placeholders
}

#[must_use]
pub fn openers() -> &'static [String] {
    &glyphs().openers
}

#[must_use]
pub fn spinner_frames() -> usize {
    glyphs().spinner.len()
}

/// What the empty prompt performs with no configuration. One line: the list lives in
/// `config/init.lua`, and this carries the `~~` marker so the performance has something to do.
const PLACEHOLDERS: [&str; 1] = ["first we need to build a ~~tool~~ tool to build the tool"];

/// What the empty prompt opens with when no configuration says otherwise.
const OPENERS: [&str; 1] = ["let's build something"];
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_box_this_has_always_drawn() {
        let g = Glyphs::default();
        assert_eq!(g.corner_top_left, "╭");
        assert_eq!(g.edge_horizontal, "─");
        assert_eq!(g.marker, "❯ ");
    }

    #[test]
    fn an_overlay_takes_what_it_is_given_and_nothing_else() {
        let mut chosen = Glyphs::default();
        chosen.overlay(&|name| (name == "marker").then(|| "▶ ".to_owned()));
        assert_eq!(chosen.marker, "▶ ");
        assert_eq!(chosen.ellipsis, "…", "and left the rest alone");
    }

    #[test]
    fn the_marker_and_its_absence_are_named_apart() {
        // They have to be the same width or every name shifts by a column when the cursor lands on it.
        assert!(Glyphs::NAMES.contains(&"marker"));
        assert!(Glyphs::NAMES.contains(&"no_marker"));
    }

    #[test]
    fn the_spinner_cycles() {
        let frames = spinner_frames();
        assert!(frames > 1, "there is something to cycle");
        assert_eq!(spinner(0), spinner(frames), "and it comes back round");
    }

    #[test]
    fn a_spinner_with_no_frames_does_not_divide_by_zero() {
        // Refused on the way in, but the reader must not be the thing that finds out.
        let empty = Glyphs {
            spinner: Vec::new(),
            ..Glyphs::default()
        };
        assert!(empty.spinner.is_empty());
    }
}
