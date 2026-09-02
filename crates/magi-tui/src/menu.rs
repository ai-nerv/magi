//! How a list of choices is drawn.
//!
//! Shared by the completion popup and the pickers, because they are the same object seen twice
//! and had drifted into two different-looking lists.
//!
//! The treatment is oslo's, less its block: **a bar and a match.** There was a background
//! behind every row, which is what made the list read as one object while it floated under the
//! prompt with nothing around it. It is drawn inside the prompt box now, and the box says where
//! the list starts and stops; a second surface inside a frame is one edge too many. The row you
//! are on keeps its bar across the full width, because that is selection and not decoration.
//! And the characters you have already typed are painted differently from the ones each
//! candidate adds, which is the single thing that makes a long list scannable: the eye is
//! looking for what is *different* between forty rows that share a prefix.

use crate::colour;
use crate::glyph;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One row of a list.
pub struct Row<'a> {
    /// What is being offered.
    pub value: &'a str,
    /// What it says about itself, to the right.
    pub detail: &'a str,
    /// Whether the cursor is on it.
    pub selected: bool,
    /// Whether it can be taken. An unready row is real, and dimmed.
    pub ready: bool,
    /// Column the value is padded to, so the details line up.
    pub value_width: usize,
}

/// Draw one row: marker, value with the typed part picked out, detail, filled to the edge.
#[must_use]
pub fn row(r: &Row<'_>, typed: &str, width: u16) -> Line<'static> {
    let bg = if r.selected {
        colour::menu_selected_bg()
    } else {
        ratatui::style::Color::Reset
    };
    let on = |style: Style| style.bg(bg);

    let value_style = if r.selected {
        Style::default()
            .fg(colour::menu_selected())
            .add_modifier(Modifier::BOLD)
    } else if r.ready {
        Style::default().fg(colour::text())
    } else {
        Style::default().fg(colour::menu_detail())
    };

    let mut spans = vec![Span::styled(
        if r.selected {
            glyph::marker()
        } else {
            glyph::no_marker()
        },
        on(Style::default().fg(colour::accent())),
    )];
    spans.extend(matched(r.value, typed, value_style, bg));

    if !r.detail.is_empty() {
        let gap = r.value_width.saturating_sub(r.value.chars().count()) + 2;
        let detail_style = if !r.ready {
            Style::default().fg(colour::warning())
        } else if r.selected {
            Style::default().fg(colour::menu_detail_selected())
        } else {
            Style::default().fg(colour::menu_detail())
        };
        spans.push(Span::styled(" ".repeat(gap), on(Style::default())));
        spans.push(Span::styled(r.detail.to_owned(), on(detail_style)));
    }

    Line::from(fill(clip(spans, usize::from(width)), width, bg))
}

/// A heading above a list: what it is, and where you are in it.
#[must_use]
pub fn heading(title: &str, note: &str, width: u16) -> Line<'static> {
    let bg = ratatui::style::Color::Reset;
    let spans = vec![
        Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(colour::accent())
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            note.to_owned(),
            Style::default().fg(colour::menu_meta()).bg(bg),
        ),
    ];
    Line::from(fill(clip(spans, usize::from(width)), width, bg))
}

/// The characters `typed` covers, in the match colour; the rest in `base`.
///
/// The literal run first, when the value contains one. That is how the row was ranked — a
/// candidate containing the word outranks one that merely has the letters in order — and it is
/// what a reader expects: typing `opus` against `openrouter/deepseek` lit `op`, `u` and `s`
/// scattered across it, which is where the matcher matched and reads as noise.
///
/// The subsequence is the fallback, for the rows that only matched that way. Marking nothing
/// there would be worse: those rows are in the list precisely because something matched, and
/// the reader is owed the reason.
fn matched(value: &str, typed: &str, base: Style, bg: ratatui::style::Color) -> Vec<Span<'static>> {
    let base = base.bg(bg);
    if typed.is_empty() {
        return vec![Span::styled(value.to_owned(), base)];
    }
    let hit = Style::default()
        .fg(colour::typed())
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let lower = value.to_lowercase();
    let wanted = typed.to_lowercase();
    if let Some(at) = lower.find(&wanted) {
        // Byte offsets from the lowercased copy index back into the original only while the two
        // agree on length, which they do for everything a model id is made of.
        if value.is_char_boundary(at) && value.is_char_boundary(at + wanted.len()) {
            let mut out = Vec::with_capacity(3);
            if at > 0 {
                out.push(Span::styled(value[..at].to_owned(), base));
            }
            out.push(Span::styled(value[at..at + wanted.len()].to_owned(), hit));
            if at + wanted.len() < value.len() {
                out.push(Span::styled(value[at + wanted.len()..].to_owned(), base));
            }
            return out;
        }
    }

    let mut wanted = typed.chars().flat_map(char::to_lowercase).peekable();
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_is_hit = false;

    for c in value.chars() {
        let is_hit = wanted
            .peek()
            .is_some_and(|w| c.to_lowercase().next() == Some(*w));
        if is_hit {
            wanted.next();
        }
        if is_hit != run_is_hit && !run.is_empty() {
            out.push(Span::styled(
                std::mem::take(&mut run),
                if run_is_hit { hit } else { base },
            ));
        }
        run_is_hit = is_hit;
        run.push(c);
    }
    if !run.is_empty() {
        out.push(Span::styled(run, if run_is_hit { hit } else { base }));
    }
    out
}

/// Truncate to `width`, keeping each span's styling.
///
/// Not `complete::fit`, which pads with an unstyled span: on a menu row that leaves the block's
/// background stopping wherever the text stops, which is the ragged edge this treatment exists
/// to avoid. Padding is [`fill`]'s job, and it pads in the row's colour.
fn clip(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0usize;
    for span in spans {
        let len = span.content.chars().count();
        if used + len <= width {
            used += len;
            out.push(span);
            continue;
        }
        let room = width - used;
        if room > 0 {
            let cut: String = span.content.chars().take(room).collect();
            out.push(Span::styled(cut, span.style));
        }
        break;
    }
    out
}

/// Pad a row to the full width so its background reaches the edge.
fn fill(
    mut spans: Vec<Span<'static>>,
    width: u16,
    bg: ratatui::style::Color,
) -> Vec<Span<'static>> {
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let room = usize::from(width).saturating_sub(used);
    if room > 0 {
        spans.push(Span::styled(" ".repeat(room), Style::default().bg(bg)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn a_row(value: &str, selected: bool) -> Row<'_> {
        Row {
            value,
            detail: "1.0M",
            selected,
            ready: true,
            value_width: value.chars().count(),
        }
    }

    #[test]
    fn an_unpicked_row_paints_no_surface_of_its_own() {
        // The prompt box is around it now, and a second surface inside a frame is one edge too
        // many. The row is still padded to the full width so the bar, when it comes, reaches it.
        let line = row(&a_row("a/b", false), "", 40);
        assert_eq!(text(&line).chars().count(), 40);
        assert!(
            line.spans
                .iter()
                .all(|s| s.style.bg == Some(ratatui::style::Color::Reset)),
            "something is painted behind an unpicked row"
        );
    }

    #[test]
    fn the_selected_row_is_the_only_one_with_a_bar() {
        let plain = row(&a_row("a/b", false), "", 40);
        let picked = row(&a_row("a/b", true), "", 40);
        assert_eq!(plain.spans[0].style.bg, Some(ratatui::style::Color::Reset));
        assert_eq!(picked.spans[0].style.bg, Some(colour::menu_selected_bg()));
        assert!(text(&picked).starts_with(glyph::marker()));
    }

    #[test]
    fn what_you_typed_is_picked_out_of_what_you_did_not() {
        // The one thing that makes forty rows sharing a prefix scannable.
        let line = row(&a_row("claude-opus-5", false), "opus", 40);
        let lit: String = line
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(colour::typed()))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(lit, "opus");
    }

    #[test]
    fn a_scattered_match_is_marked_where_it_actually_matched() {
        // Matched as a subsequence, because that is how the candidate was chosen.
        let line = row(&a_row("deepseek-v4-flash", false), "v4", 40);
        let lit: String = line
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(colour::typed()))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(lit, "v4");
    }

    #[test]
    fn nothing_typed_lights_nothing() {
        let line = row(&a_row("anything", false), "", 40);
        assert!(
            line.spans
                .iter()
                .all(|s| s.style.fg != Some(colour::typed()))
        );
    }

    #[test]
    fn an_unready_row_says_so_in_the_detail_colour() {
        let r = Row {
            value: "anthropic/x",
            detail: "set ANTHROPIC_API_KEY",
            selected: false,
            ready: false,
            value_width: 11,
        };
        let line = row(&r, "", 60);
        assert!(
            line.spans
                .iter()
                .any(|s| s.style.fg == Some(colour::warning())),
            "the reason stands out from the size"
        );
    }

    #[test]
    fn a_heading_fills_the_width_too() {
        let line = heading("Model", "  3 of 40", 40);
        assert_eq!(text(&line).chars().count(), 40);
        assert!(text(&line).contains("Model"));
    }

    #[test]
    fn a_narrow_terminal_does_not_overflow() {
        for w in 1..12_u16 {
            let line = row(&a_row("provider/model", true), "mod", w);
            assert!(text(&line).chars().count() <= usize::from(w), "width {w}");
        }
    }
}

#[cfg(test)]
mod match_tests {
    use super::*;

    fn lit(value: &str, typed: &str) -> String {
        let r = Row {
            value,
            detail: "",
            selected: false,
            ready: true,
            value_width: value.chars().count(),
        };
        row(&r, typed, 80)
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(colour::typed()))
            .map(|s| s.content.as_ref())
            .collect()
    }

    /// How many separate lit runs there are, which is what tells a literal match from a
    /// scattered one: both collect to the same characters.
    fn runs(value: &str, typed: &str) -> usize {
        let r = Row {
            value,
            detail: "",
            selected: false,
            ready: true,
            value_width: value.chars().count(),
        };
        row(&r, typed, 80)
            .spans
            .iter()
            .filter(|s| s.style.fg == Some(colour::typed()))
            .count()
    }

    #[test]
    fn the_literal_word_is_lit_as_one_run() {
        // Not `op` + `u` + `s` scattered down the row, which is where the fuzzy matcher matched
        // and reads as noise.
        assert_eq!(lit("openrouter/anthropic/claude-opus-5", "opus"), "opus");
        assert_eq!(runs("openrouter/anthropic/claude-opus-5", "opus"), 1);
    }

    #[test]
    fn a_row_that_only_matched_loosely_still_shows_why() {
        // Those rows are in the list because something matched; marking nothing would owe the
        // reader an explanation the list cannot give. It is several runs rather than one,
        // which is the visible difference from a literal hit.
        assert!(runs("openrouter/deepseek/deepseek-v3.2", "opus") > 1);
    }
    #[test]
    fn matching_ignores_case() {
        assert_eq!(lit("Claude-OPUS-5", "opus"), "OPUS");
    }

    #[test]
    fn a_match_at_the_very_start_keeps_the_rest() {
        let r = Row {
            value: "opus-5",
            detail: "",
            selected: false,
            ready: true,
            value_width: 6,
        };
        let text: String = row(&r, "opus", 40)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("opus-5"), "{text}");
    }
}
