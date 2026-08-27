//! What an empty session shows.
//!
//! Opening axum in a fresh directory drew twenty-five blank rows and an empty box. There was
//! nothing to say it had started, nothing to say which model it would use, and nothing to say
//! that `/` opens a command list — the two questions a person has on the first screen, and the
//! one key that answers the rest.
//!
//! Deliberately small. This is a prompt with a label on it, not a splash screen: it occupies
//! the space a transcript is about to, and the first message pushes it out of the way for good.

use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The keys worth knowing before there is any transcript to act on.
const HINTS: &[(&str, &str)] = &[
    ("/", "commands"),
    ("@", "a file"),
    ("↵", "send"),
    ("^c", "quit"),
];

/// Render the greeting for a session that has nothing in it yet.
///
/// `model` and `cwd` are what the footer already knows; they are repeated here because the
/// footer is two dim lines at the bottom edge and this is the middle of the screen, which is
/// where a person is looking when they have just started.
#[must_use]
pub fn render(model: &str, cwd: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme.dim);
    let muted = Style::default().fg(theme.muted);

    let mut out = vec![
        Line::from(Span::styled(
            "axum",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // The sentinel is not a name. Printing `no-model` here reads as a model called that, and
    // the notice below already says what to do about it.
    if !model.is_empty() && model != crate::footer::NO_MODEL {
        out.push(Line::from(Span::styled(
            crate::footer::fit_path(model, usize::from(width)),
            muted,
        )));
    }
    if !cwd.is_empty() {
        out.push(Line::from(Span::styled(
            crate::footer::fit_path(cwd, usize::from(width)),
            dim,
        )));
    }
    out.push(Line::from(""));

    // Whole pairs only. A line cut mid-hint drops `^c quit` without saying it did, and the
    // key a stuck reader most needs is the one at the end.
    let mut hints: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for (key, what) in HINTS {
        let sep = if hints.is_empty() { 0 } else { 3 };
        let pair = key.chars().count() + 1 + what.chars().count();
        if used + sep + pair > usize::from(width) {
            break;
        }
        if sep > 0 {
            hints.push(Span::styled("   ", dim));
        }
        hints.push(Span::styled((*key).to_owned(), muted));
        hints.push(Span::styled(format!(" {what}"), dim));
        used += sep + pair;
    }
    out.push(Line::from(hints));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn it_says_what_it_is_and_what_it_will_use() {
        let out = render(
            "openrouter/deepseek",
            "/work/thing",
            80,
            &crate::theme::DARK,
        );
        let text = text_of(&out);
        assert!(text.contains("axum"), "{text}");
        assert!(text.contains("openrouter/deepseek"), "{text}");
        assert!(text.contains("/work/thing"), "{text}");
    }

    #[test]
    fn it_names_the_key_that_finds_the_rest() {
        // The complaint that started this was not being able to reach the model list.
        let out = render("m", "/w", 80, &crate::theme::DARK);
        let text = text_of(&out);
        assert!(text.contains("/ commands"), "{text}");
    }

    #[test]
    fn a_narrow_terminal_still_gets_a_path_it_can_read() {
        let out = render(
            "m",
            "/home/someone/work/deeply/nested/project",
            20,
            &crate::theme::DARK,
        );
        let text = text_of(&out);
        assert!(
            text.contains("project"),
            "the tail is the part that identifies it: {text}"
        );
    }

    #[test]
    fn no_model_yet_is_not_an_empty_line() {
        // Before the daemon answers, the model is unknown; a blank row reads as a bug.
        let out = render("", "/w", 80, &crate::theme::DARK);
        let text = text_of(&out);
        assert!(!text.contains("\n\n\n"), "{text}");
    }
}

#[cfg(test)]
mod narrow_tests {
    use super::*;

    fn hints_row(width: u16) -> String {
        let out = render("m", "/w", width, &crate::theme::DARK);
        out.last()
            .expect("a line")
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn hints_are_dropped_whole_rather_than_cut() {
        // A line cut mid-hint drops `^c quit` without saying it did.
        let row = hints_row(30);
        assert!(row.chars().count() <= 30, "{row:?}");
        for fragment in ["quit", "send", "a file"] {
            if row.contains(fragment) {
                continue;
            }
            assert!(!row.ends_with(&fragment[..2]), "half a hint: {row:?}");
        }
    }

    #[test]
    fn the_first_hint_is_the_one_that_survives() {
        // `/` is the key that finds everything else.
        let row = hints_row(14);
        assert!(row.contains("/ commands"), "{row:?}");
    }

    #[test]
    fn a_wide_terminal_keeps_them_all() {
        assert!(hints_row(80).contains("quit"));
    }
}
