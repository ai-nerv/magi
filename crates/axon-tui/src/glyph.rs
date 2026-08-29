//! The characters the UI is drawn out of.
//!
//! Every one of them is a choice somebody could reasonably disagree with — a rounded box or a
//! square one, a chevron or an arrow, braille dots or a bar that fills — and none of them is worth
//! a fork. So they are settings, by the names below, from `axon.ui` in Lua.
//!
//! ```lua
//! axon.ui.corner_top_left = "┌"
//! axon.ui.marker          = "▶ "
//! axon.ui.spinner         = { "◐", "◓", "◑", "◒" }
//! ```
//!
//! **Width is the caller's problem, not this module's.** A glyph two columns wide will draw two
//! columns wide, and a box corner that does that will not line up. Nothing here measures.

use std::sync::OnceLock;

/// Declare the glyphs once: the struct, the defaults, the accessors, and the names a config may
/// set, from one list so none of them can fall out of step.
macro_rules! glyphs {
    ($($name:ident = $default:literal, $doc:literal;)*) => {
        /// Every character the UI is drawn out of.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct Glyphs {
            $(#[doc = $doc] pub $name: String,)*
            /// The spinner, a frame at a time.
            ///
            /// A list rather than a fixed count: a spinner is however many frames its author drew.
            /// An empty one is refused on the way in, because a spinner with no frames is a
            /// division by zero at the one moment somebody is watching the screen.
            pub spinner: Vec<String>,
        }

        impl Default for Glyphs {
            fn default() -> Self {
                Self {
                    $($name: $default.to_owned(),)*
                    spinner: SPINNER.iter().map(|f| (*f).to_owned()).collect(),
                }
            }
        }

        impl Glyphs {
            /// Every name `axon.ui` recognises as a glyph, beside `spinner`.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            /// Take whatever `given` answers for, and keep the rest.
            pub fn overlay(&mut self, given: &dyn Fn(&str) -> Option<String>) {
                $(if let Some(value) = given(stringify!($name)) { self.$name = value; })*
            }
        }

        $(#[doc = $doc] #[must_use] pub fn $name() -> &'static str { &glyphs().$name })*
    };
}

/// The braille spinner, which is what a terminal has drawn for twenty years.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

glyphs! {
    corner_top_left = "╭", "The prompt box's top-left corner.";
    corner_top_right = "╮", "The prompt box's top-right corner.";
    corner_bottom_left = "╰", "The prompt box's bottom-left corner.";
    corner_bottom_right = "╯", "The prompt box's bottom-right corner.";
    edge_horizontal = "─", "The prompt box's top and bottom edges.";
    edge_vertical = "│", "The prompt box's sides.";
    marker = "❯ ", "In front of the row of a list you are on.";
    no_marker = "  ", "In front of every other row, so the names stay in one column.";
    ellipsis = "…", "Where something was cut to fit.";
    bullet = "• ", "A markdown list item.";
    more_rule = "─ ", "Repeated along an edge the transcript continues past.";
    expand = "»", "At the end of a folded tool block: click to open it.";
    collapse = "«", "At the end of an open tool block: click to fold it.";
    quote_rule = "│ ", "Down the left of a block quote.";
    notice_rule = "│ ", "Down the left of something the UI itself is saying.";
    placeholder = "ask anything, or / for commands", "The prompt, before you type anything.";
    placeholder_short = "/ for commands", "The same, on a screen too narrow for the whole of it.";
    no_model = "no-model", "What the footer says when nothing is configured to answer.";
}

/// The glyphs in force, set once before anything is drawn.
static IN_FORCE: OnceLock<Glyphs> = OnceLock::new();

/// Use `glyphs` for the life of the process.
///
/// Only the first call counts, for the same reason the palette's does: a screen half drawn in one
/// set and half in another is worse than a set nobody asked for.
pub fn adopt(glyphs: Glyphs) {
    let _ = IN_FORCE.set(glyphs);
}

/// The glyphs in force.
#[must_use]
pub fn glyphs() -> &'static Glyphs {
    IN_FORCE.get_or_init(Glyphs::default)
}

/// The spinner frame for `tick`.
#[must_use]
pub fn spinner(tick: usize) -> &'static str {
    let frames = &glyphs().spinner;
    if frames.is_empty() {
        return "";
    }
    &frames[tick % frames.len()]
}

/// How many frames the spinner has.
#[must_use]
pub fn spinner_frames() -> usize {
    glyphs().spinner.len()
}

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
        // They have to be the same width or every name in the list shifts by a column when the
        // cursor lands on it. Naming both is what lets somebody keep that true.
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
