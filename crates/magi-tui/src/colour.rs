//! The colours. Text hues are fixed RGB, bright enough to read off a dark screen and the same
//! whatever a theme tool does to the terminal's sixteen. Greys, rules and surfaces are indices into
//! the terminal's own palette — the top fifth of the 232-255 greyscale — so backgrounds follow the
//! theme. Every one is settable by name from `magi.ui` in Lua, as an index or as `"#rrggbb"`.
//!
//! ```lua
//! magi.ui.accent = "#bd93f9"
//! magi.ui.muted  = 8
//! ```

use ratatui::style::Color;
use std::sync::OnceLock;

/// The text hues: bright, and fixed, so a theme that repaints the terminal's sixteen leaves them be.
const GREEN: Color = Color::Rgb(0x5a, 0xf7, 0x8e);
const VIOLET: Color = Color::Rgb(0xbd, 0x93, 0xf9);
const BLUE: Color = Color::Rgb(0x57, 0xc7, 0xff);
const ORANGE: Color = Color::Rgb(0xff, 0xb8, 0x6c);
const PINK: Color = Color::Rgb(0xff, 0x6a, 0xc1);
const CYAN: Color = Color::Rgb(0x8b, 0xe9, 0xfd);
const YELLOW: Color = Color::Rgb(0xf3, 0xf9, 0x9d);
const RED: Color = Color::Rgb(0xff, 0x5c, 0x57);

/// Declare the palette once — struct, defaults, accessors and the names a config may set — from one list.
macro_rules! palette {
    ($($name:ident = $default:expr, $doc:literal;)*) => {
        /// Every colour the UI draws with.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Palette {
            $(#[doc = $doc] pub $name: Color,)*
        }

        pub const STOCK: Palette = Palette { $($name: $default,)* };

        impl Palette {
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            /// Take whatever `given` answers for; a name it has no answer for keeps its default.
            pub fn overlay(&mut self, given: &dyn Fn(&str) -> Option<Color>) {
                $(if let Some(value) = given(stringify!($name)) { self.$name = value; })*
            }
        }

        $(#[doc = $doc] #[must_use] pub fn $name() -> Color { palette().$name })*
    };
}

// Secondary text lives in the 246-251 band and surfaces sit above 236: below that a menu row is
// barely above the screen.
palette! {
    accent = VIOLET, "Spinners, list cursors, markdown bullets.";
    success = GREEN, "Success states.";
    warning = ORANGE, "Warnings and elevated context usage.";
    error = RED, "Errors, and a tool that failed.";
    typed = YELLOW, "The characters you have already typed, wherever they appear in a candidate.";
    spinning = VIOLET, "The status row's display while a turn runs; at rest it is `dim`.";
    mode_normal = BLUE, "`NOR` on the prompt box: keys are commands.";
    mode_insert = GREEN, "`INS` on the prompt box: keys are text.";
    mode_command = ORANGE, "`CMD` on the prompt box: keys are a command line.";

    code_command = BLUE, "The program a shell command runs, and each one after a pipe or `&&`.";
    code_subcommand = VIOLET, "What a program like `git` or `cargo` is asked to do: `commit`, `build`.";
    code_flag = CYAN, "A flag on a shell command: `-la`, `--oneline`.";
    code_path = YELLOW, "A path on a shell command: `./src`, `~/x`, `a.rs`, a glob.";
    code_number = ORANGE, "A number on a shell command.";
    code_string = GREEN, "A quoted string in a shell command.";
    code_variable = ORANGE, "`$NAME`, `${NAME}`, and `NAME=value` before a command.";
    code_operator = PINK, "`|`, `&&`, `;`, `>` and the other joints between commands.";
    code_comment = at(245), "A trailing `#` comment on a shell command.";
    code_argument = at(253), "Everything else on a shell command: its plain arguments.";
    code_keyword = VIOLET, "A keyword in a code block: `fn`, `if`, `return`.";
    code_type = CYAN, "A type or class name in a code block.";

    md_heading = ORANGE, "Markdown headings.";
    md_code = CYAN, "Inline code spans.";
    md_code_block = GREEN, "Fenced code block contents.";
    md_quote = at(250), "Block quote text and its rule.";

    diff_added = GREEN, "Added lines in a diff.";
    diff_removed = RED, "Removed lines in a diff.";
    diff_marker = VIOLET, "A diff's file and hunk headers, which are neither added nor removed.";
    diff_context = at(245), "Unchanged context lines in a diff.";
    diff_added_bg = at(22), "Behind a line an edit added.";
    diff_removed_bg = at(52), "Behind a line an edit removed.";
    diff_changed_bg = at(94), "Behind the new side of a changed line: added straight after removed ones.";

    tool_bg = at(237), "Behind a tool block.";
    tool_title = BLUE, "The tool's name, while the call is out.";
    tool_ok = GREEN, "The tool's name, when it finished.";
    tool_failed = RED, "The tool's name, when it failed.";
    tool_output = at(251), "A tool's output.";
    tool_fold = at(246), "The note saying how much of a result is not shown.";
    tool_seam = at(235), "The rule between what a call was asked and what it answered. A line rather than a surface, so it may sit below the floor a fill has to keep.";
    block_frame = at(237), "A transcript block's own frame. Not the prompt's border: a box in a scrolling record should sit further back than the thing you are typing into.";

    menu_selected_bg = at(241), "Behind the row you are on.";
    menu_selected = at(255), "The row you are on.";
    menu_detail = at(250), "What a row says about itself, beside its name.";
    menu_detail_selected = at(255), "The same, on the selected row.";
    menu_meta = at(247), "Counts and scroll markers on the heading.";
    pane_selected_bg = at(237), "Behind the entry a list float's cursor is on.";

    // Two ends of one gradient: the scan walks the greyscale indices between them.
    border = at(240), "The prompt's border with nothing lit, and the floor of its scan.";
    scan = at(255), "The brightest point of the light travelling along the border.";
    hint = at(241), "The empty prompt's placeholder. Well under the text, so it reads as a label rather than as something you wrote.";
    shimmer_shadow = at(235), "What the working band darkens the words in the prompt box towards, where it passes.";
    rule = at(245), "The rule above and below a quotation.";

    message_bg = at(237), "Behind something you said.";
    message_text = at(255), "Something you said.";
    said_by_you = PINK, "The `USER` tag on something you said.";
    said_by_agent = CYAN, "The tag on a message from another instance.";
    thinking = at(249), "Reasoning blocks.";
    text = at(253), "Default foreground.";
    muted = at(250), "Secondary text.";
    dim = at(246), "Tertiary text; the footer lives here.";
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

/// A colour as a configuration writes one: a palette index, or `"#rrggbb"`. Anything else is
/// `None`, so a mistake stays visible rather than being painted over.
#[must_use]
pub fn read(index: Option<u64>, text: Option<&str>) -> Option<Color> {
    if let Some(index) = index {
        return u8::try_from(index).ok().map(Color::Indexed);
    }
    let hex = text?.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |from: usize| u8::from_str_radix(hex.get(from..from + 2)?, 16).ok();
    Some(Color::Rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// `from` moved `t` of the way to `to`. Only colours with a known RGB value blend — the fixed hues
/// and the greyscale; anything else is whichever end `t` is nearer.
#[must_use]
pub fn blend(from: Color, to: Color, t: f32) -> Color {
    let (Some(a), Some(b)) = (rgb_of(from), rgb_of(to)) else {
        return if t < 0.5 { from } else { to };
    };
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| {
        let mixed = f32::from(x) + (f32::from(y) - f32::from(x)) * t;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "between two bytes"
        )]
        let byte = mixed.round() as u8;
        byte
    };
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// `from` taken `t` of the way to `to` without leaving the terminal's palette: two greyscale steps
/// walk the indices between them, as the scan does the other way, so a theme's greys stay its own.
/// RGB blends; any other index cannot be mixed, and past halfway is itself, dimmed.
#[must_use]
pub fn shade(from: Color, to: Color, t: f32) -> ratatui::style::Style {
    let t = t.clamp(0.0, 1.0);
    let style = ratatui::style::Style::default();
    match (from, to) {
        (Color::Indexed(a @ 232..=255), Color::Indexed(b @ 232..=255)) => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "at most 23 steps"
            )]
            let step = (f32::from(a.abs_diff(b)) * t).round() as u8;
            style.fg(Color::Indexed(if b < a { a - step } else { a + step }))
        }
        _ if rgb_of(from).is_some() && rgb_of(to).is_some() => style.fg(blend(from, to, t)),
        _ if t >= 0.5 => style.fg(from).add_modifier(ratatui::style::Modifier::DIM),
        _ => style.fg(from),
    }
}

/// The RGB a colour stands for, where that is fixed: an RGB hue, or a step of the 232-255 greyscale.
fn rgb_of(colour: Color) -> Option<(u8, u8, u8)> {
    match colour {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(n @ 232..=255) => {
            let grey = 8 + 10 * (n - 232);
            Some((grey, grey, grey))
        }
        _ => None,
    }
}

/// `amount` of the way from the resting border to the brightest point of the scan: a step along the
/// run of greyscale indices between them. Two colours that are not both indices have no run to
/// walk, and a palette whose scan is not above its border gets the border.
#[must_use]
pub fn scan_at(amount: f32) -> Color {
    let Palette { border, scan, .. } = *palette();
    let amount = amount.clamp(0.0, 1.0);
    let (Color::Indexed(low), Color::Indexed(high)) = (border, scan) else {
        return if amount >= 0.5 { scan } else { border };
    };
    let Some(span) = high.checked_sub(low) else {
        return border;
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the run first"
    )]
    let step = (f32::from(span) * amount).round() as u8;
    at(low + step)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour's place on the terminal's palette; a test asking this of an RGB hue is wrong.
    fn index(colour: Color) -> u8 {
        match colour {
            Color::Indexed(n) => n,
            other => panic!("{other:?} is not on the terminal's palette"),
        }
    }

    #[test]
    fn nothing_lit_is_the_resting_border() {
        assert_eq!(scan_at(0.0), border());
    }

    #[test]
    fn fully_lit_is_the_top_of_the_run() {
        assert_eq!(scan_at(1.0), palette().scan);
    }

    #[test]
    fn the_scan_only_climbs() {
        let seen: Vec<u8> = (0..=10u8)
            .map(|n| index(scan_at(f32::from(n) / 10.0)))
            .collect();
        for pair in seen.windows(2) {
            assert!(pair[0] <= pair[1], "it only climbs: {pair:?}");
        }
    }

    #[test]
    fn an_amount_off_the_end_stays_on_the_run() {
        assert_eq!(scan_at(-1.0), border());
        assert_eq!(scan_at(9.0), palette().scan);
    }

    #[test]
    fn an_overlay_takes_what_it_is_given_and_nothing_else() {
        let mut chosen = STOCK;
        chosen.overlay(&|name| (name == "accent").then_some(at(1)));
        assert_eq!(chosen.accent, at(1));
        assert_eq!(chosen.muted, STOCK.muted, "and left the rest alone");
    }

    #[test]
    fn every_field_can_be_named() {
        // The macro builds struct, defaults, accessors and this list from one place. Counted rather
        // than compared because there is nothing else to compare it against.
        let mut all = STOCK;
        all.overlay(&|_| Some(at(200)));
        assert_eq!(all.accent, at(200));
        assert_eq!(all.scan, at(200));
        assert!(Palette::NAMES.len() > 25, "{}", Palette::NAMES.len());
        assert!(Palette::NAMES.contains(&"tool_output"));
    }

    #[test]
    fn a_colour_is_read_as_an_index_or_as_rgb() {
        assert_eq!(read(Some(1), None), Some(at(1)));
        assert_eq!(
            read(None, Some("#ff8800")),
            Some(Color::Rgb(0xff, 0x88, 0x00))
        );
        assert_eq!(read(Some(300), None), None, "past the palette");
        assert_eq!(read(None, Some("grey")), None, "not a colour");
        assert_eq!(read(None, Some("#ff880")), None, "one digit short");
        assert_eq!(read(None, Some("#gg8800")), None, "not hex");
    }

    #[test]
    fn the_hues_are_fixed_and_the_surfaces_follow_the_theme() {
        // A theme tool rewrites the terminal's sixteen: text drawn from them changes colour under
        // it, and a background that did not would sit wrong on the new screen.
        for hue in [
            STOCK.accent,
            STOCK.success,
            STOCK.warning,
            STOCK.error,
            STOCK.typed,
            STOCK.spinning,
            STOCK.mode_normal,
            STOCK.tool_title,
            STOCK.said_by_you,
        ] {
            assert!(matches!(hue, Color::Rgb(..)), "{hue:?} is not fixed");
        }
        for surface in [
            STOCK.tool_bg,
            STOCK.menu_selected_bg,
            STOCK.message_bg,
            STOCK.pane_selected_bg,
            STOCK.border,
        ] {
            assert!(index(surface) > 236, "{surface:?} is as good as black");
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
            assert!(
                index(weight) >= 246,
                "{weight:?} is too dark to read comfortably"
            );
        }
        // `hint` is deliberately not in that list: a placeholder as bright as what you type reads
        // as something already in the box.
        let hint = index(STOCK.hint);
        assert!(hint < 246, "the placeholder is as loud as the text: {hint}");
        assert!(hint > 236, "and not a hole in the screen: {hint}");
    }

    #[test]
    fn the_scan_has_a_run_long_enough_to_read_as_one() {
        // Twelve steps is the floor at which a comet reads as a comet rather than two greys.
        let run = index(STOCK.scan).saturating_sub(index(STOCK.border));
        assert!(
            run >= 12,
            "only {run} steps between the border and the scan"
        );
    }

    #[test]
    fn text_weights_are_ordered() {
        let (dim, muted, text) = (index(STOCK.dim), index(STOCK.muted), index(STOCK.text));
        assert!(dim < muted, "the footer is quieter than output");
        assert!(muted < text, "output is quieter than prose");
        assert!(
            text < index(STOCK.menu_selected),
            "a selected row beats prose"
        );
    }
}
