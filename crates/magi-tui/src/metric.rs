//! The numbers: how big things are, and how fast they move.
//!
//! Rows, budgets, fractions and rates, all of them settable by the names below from `magi.ui` in
//! Lua. They were constants scattered across a dozen modules, which is fine right up until
//! somebody wants a taller transcript or a shorter tool preview and has to fork to get one.
//!
//! ```lua
//! magi.ui.menu_rows     = 12
//! magi.ui.preview_lines = 20
//! magi.ui.scan_speed    = 2
//! ```
//!
//! **A fraction is a percentage**, because Lua has one number type and a config that says `0.3`
//! and a config that says `30` should not mean different things by accident. `scan_speed` is the
//! exception and is documented as a multiplier, because "twice as fast" is what somebody means.

use std::sync::OnceLock;

/// Declare the numbers once: the struct, the defaults, the accessors, and the names a config may
/// set, from one list so none of them can fall out of step.
macro_rules! metrics {
    ($($name:ident: $kind:ty = $default:literal, $floor:literal, $doc:literal;)*) => {
        /// Every size and rate the UI draws by.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Metrics {
            $(#[doc = $doc] pub $name: $kind,)*
        }

        /// What the UI was built around, before any of it was settable.
        pub const BUILT_IN: Metrics = Metrics { $($name: $default,)* };

        impl Metrics {
            /// Every name `magi.ui` recognises as a number.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            /// Take whatever `given` answers for, and keep the rest.
            ///
            /// Each has a floor, and a value under it is raised rather than refused. Zero rows of
            /// menu is not a preference, it is a menu that cannot be seen, and the person who
            /// typed it is not at a debugger.
            pub fn overlay(&mut self, given: &dyn Fn(&str) -> Option<u64>) {
                $(if let Some(value) = given(stringify!($name)) {
                    self.$name = <$kind>::try_from(value).unwrap_or(<$kind>::MAX).max($floor);
                })*
            }
        }

        $(#[doc = $doc] #[must_use] pub fn $name() -> $kind { metrics().$name })*
    };
}

metrics! {
    // ------------------------------------------------------------ the rows
    footer_rows: u16 = 1, 0, "Rows the footer always occupies.";
    prompt_min_rows: u16 = 3, 3, "Rows the prompt claims holding a single line: rule, text, rule.";
    live_rows: u16 = 10, 1, "Live transcript rows to aim for, before the terminal's height.";
    live_share: u16 = 34, 5, "Most of the screen the live region may claim, as a percentage.";
    menu_rows: u16 = 8, 1, "Rows of a list or a completion shown at once.";
    preview_lines: u16 = 10, 1, "Lines of a tool result shown before it is expanded.";
    prompt_share: u16 = 30, 5, "Most of the screen the prompt may claim, as a percentage.";
    prompt_min_lines: u16 = 5, 1, "Lines of the prompt shown however small the screen is.";
    page_share: u16 = 50, 10, "How much of a screen `page up` moves, as a percentage.";

    // -------------------------------------------------------- the spacings
    block_pad: u16 = 1, 0, "Columns of padding inside a tool block or a message.";
    gutter: u16 = 2, 0, "Columns the prompt box takes before its text: the bar, then padding.";
    tab_width: u16 = 4, 1, "Columns a tab expands to.";
    column_gap: u16 = 2, 1, "Columns between a name and what it says about itself.";
    min_column: u16 = 3, 1, "Narrowest a table column is squeezed to before it is cut.";

    // ------------------------------------------------------- the summaries
    summary_budget: u16 = 72, 8, "Columns a tool call's arguments share in a block header.";
    argument_floor: u16 = 12, 4, "Columns any one argument gets, however many there are.";

    // ---------------------------------------------------------- the motion
    frame_ms: u64 = 80, 16, "Milliseconds between frames.";
    scan_speed: u16 = 100, 0, "How fast the border's light moves, as a percentage of built-in.";
    scan_nose: u16 = 3, 1, "Cells of the border lit ahead of a travelling light.";
    scan_tail: u16 = 10, 1, "Cells of the border lit behind one.";
    rest_pace: u16 = 133, 1, "Cells per frame at rest, as a percentage.";
    hold_pace: u16 = 200, 1, "Cells per frame with something typed, as a percentage.";
    work_pace: u16 = 400, 1, "Cells per frame while a turn runs, as a percentage.";
    decrypt_ms: u64 = 0, 0, "How long the opening scramble runs. Zero, the default, is no effect.";
    flicker_odds: u16 = 0, 0, "One box character in this many glitches per window. Zero is off.";
    flicker_ms: u64 = 120, 16, "How long one glitched character holds before it comes back.";
    type_reveal_ms: u64 = 0, 0, "How long a character you type takes to resolve. Zero is off.";
    tease_after_ms: u64 = 30_000, 0, "Idle time before the empty prompt starts writing to itself. Zero is off.";
    tease_step_ms: u64 = 45, 1, "How long one character of that takes.";
    tease_doubt_ms: u64 = 900, 0, "How long the word it regrets sits there before it is taken back.";
    beacon_ms: u64 = 2_000, 1, "How long the trace takes to scroll one display width.";
    beacon_cells: u16 = 9, 1, "How many braille cells wide that display is. Odd, so it has a middle.";
    footer_pad: u16 = 3, 0, "Columns held clear at each end of the footer.";
}

impl Default for Metrics {
    fn default() -> Self {
        BUILT_IN
    }
}

/// The metrics in force, set once before anything is drawn.
static IN_FORCE: OnceLock<Metrics> = OnceLock::new();

/// Use `metrics` for the life of the process.
pub fn adopt(metrics: Metrics) {
    let _ = IN_FORCE.set(metrics);
}

/// The metrics in force.
#[must_use]
pub fn metrics() -> &'static Metrics {
    IN_FORCE.get_or_init(Metrics::default)
}

/// The value a percentage setting has when nobody has changed it.
///
/// Named because two things divide by it — the scan clock and every `share` — and a literal 100
/// in either of them is a number whose meaning has to be worked out from context.
pub const NORMAL: u16 = 100;

/// `share` percent of `whole`, at least one.
///
/// Percentages rather than floats, so a config saying `30` and a config saying `0.3` cannot mean
/// different things by accident — Lua has one number type and no way to tell them apart.
#[must_use]
pub fn share(whole: u16, share: u16) -> u16 {
    (whole.saturating_mul(share) / NORMAL).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_what_this_was_built_around() {
        assert_eq!(BUILT_IN.menu_rows, 8);
        assert_eq!(BUILT_IN.preview_lines, 10);
        assert_eq!(BUILT_IN.frame_ms, 80);
    }

    #[test]
    fn an_overlay_takes_what_it_is_given_and_nothing_else() {
        let mut chosen = BUILT_IN;
        chosen.overlay(&|name| (name == "menu_rows").then_some(20));
        assert_eq!(chosen.menu_rows, 20);
        assert_eq!(chosen.preview_lines, BUILT_IN.preview_lines);
    }

    #[test]
    fn a_value_under_the_floor_is_raised_rather_than_drawn() {
        // Zero rows of menu is not a preference, it is a menu that cannot be seen.
        let mut chosen = BUILT_IN;
        chosen.overlay(&|name| (name == "menu_rows").then_some(0));
        assert_eq!(chosen.menu_rows, 1);
    }

    #[test]
    fn a_value_too_big_for_its_type_saturates_rather_than_wrapping() {
        // `as` would have made 65536 rows into none at all.
        let mut chosen = BUILT_IN;
        chosen.overlay(&|name| (name == "menu_rows").then_some(999_999));
        assert_eq!(chosen.menu_rows, u16::MAX);
    }

    #[test]
    fn a_share_is_a_percentage_and_never_nothing() {
        assert_eq!(share(90, 33), 29);
        assert_eq!(share(2, 1), 1, "a rounding error is not an empty region");
    }

    #[test]
    fn the_scan_can_be_stopped_but_not_the_frames() {
        // Zero speed is a still border, which somebody may want. Zero milliseconds between
        // frames is a busy loop, which nobody does.
        let mut chosen = BUILT_IN;
        chosen.overlay(&|_| Some(0));
        assert_eq!(chosen.scan_speed, 0);
        assert_eq!(chosen.frame_ms, 16);
    }
}
