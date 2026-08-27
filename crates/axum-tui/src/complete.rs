//! Prompt autocompletion.
//!
//! Two triggers, as in Pi: `/` at the start of the prompt opens the command palette, and `@`
//! anywhere completes a path. Both render as an overlay above the prompt and are driven from
//! the editor's current line, so nothing here holds state the editor already owns.

use crate::fuzzy;
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Rows the overlay will use at most, so a long candidate list cannot eat the screen.
pub const MAX_VISIBLE: usize = 8;

/// What is being completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A slash command at the start of the prompt.
    Command,
    /// A file path after an `@`.
    Path,
}

/// One offered completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Text inserted when accepted.
    pub value: String,
    /// One-line explanation, shown muted beside the value.
    pub detail: String,
}

/// An open completion popup.
#[derive(Debug, Clone)]
pub struct Completion {
    /// What is being completed.
    pub kind: Kind,
    /// Ranked candidates, best first.
    pub candidates: Vec<Candidate>,
    /// Which candidate is highlighted.
    pub selected: usize,
    /// Character index in the line where the replaced token starts.
    pub token_start: usize,
}

impl Completion {
    /// The highlighted candidate.
    #[must_use]
    pub fn current(&self) -> Option<&Candidate> {
        self.candidates.get(self.selected)
    }

    /// Move the highlight down, wrapping.
    pub fn next(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected + 1) % self.candidates.len();
        }
    }

    /// Move the highlight up, wrapping.
    pub fn prev(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.candidates.len() - 1);
        }
    }

    /// Rows the overlay needs.
    #[must_use]
    pub fn height(&self) -> u16 {
        self.candidates.len().min(MAX_VISIBLE) as u16
    }
}

/// The slash commands M0 can honour.
///
/// Deliberately short. Pi has 28 and a collision policy per surface; every command added here
/// is a capability the daemon must eventually answer for.
#[must_use]
pub fn commands() -> Vec<Candidate> {
    [
        ("/help", "show keybindings and commands"),
        ("/clear", "clear the transcript"),
        ("/quit", "exit axum"),
    ]
    .iter()
    .map(|(value, detail)| Candidate {
        value: (*value).to_owned(),
        detail: (*detail).to_owned(),
    })
    .collect()
}

/// Work out what, if anything, should be completed for `line` with the cursor at `col`.
///
/// `list_paths` supplies path candidates for a prefix; it is a parameter so this stays a pure
/// function and the filesystem lives in the caller.
pub fn resolve(
    line: &str,
    col: usize,
    list_paths: &dyn Fn(&str) -> Vec<String>,
) -> Option<Completion> {
    let before: String = line.chars().take(col).collect();

    if let Some(query) = before.strip_prefix('/')
        && !query.contains(char::is_whitespace)
    {
        {
            let all = commands();
            let values: Vec<String> = all.iter().map(|c| c.value.clone()).collect();
            let ranked = fuzzy::filter(&format!("/{query}"), &values);
            let candidates = ranked
                .into_iter()
                .filter_map(|v| all.iter().find(|c| &c.value == v).cloned())
                .collect::<Vec<_>>();
            return (!candidates.is_empty()).then_some(Completion {
                kind: Kind::Command,
                candidates,
                selected: 0,
                token_start: 0,
            });
        }
    }

    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    if query.contains(char::is_whitespace) {
        return None;
    }

    let paths = list_paths(query);
    let ranked = fuzzy::filter(query, &paths);
    let candidates: Vec<Candidate> = ranked
        .into_iter()
        .take(MAX_VISIBLE * 4)
        .map(|value| Candidate {
            value: value.clone(),
            detail: String::new(),
        })
        .collect();

    (!candidates.is_empty()).then(|| Completion {
        kind: Kind::Path,
        candidates,
        selected: 0,
        // The `@` itself is replaced along with the query, so accepting a path leaves a bare
        // path in the prompt rather than one still wearing its trigger.
        token_start: before[..at].chars().count(),
    })
}

/// Render the overlay.
#[must_use]
pub fn render(completion: &Completion, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let window = window(completion);
    // Details line up in a column: a ragged left edge on the descriptions makes the list read
    // as noise rather than as a table of choices.
    let value_width = completion.candidates[window.clone()]
        .iter()
        .map(|c| c.value.chars().count())
        .max()
        .unwrap_or(0);

    completion.candidates[window.clone()]
        .iter()
        .enumerate()
        .map(|(offset, candidate)| {
            let index = window.start + offset;
            let selected = index == completion.selected;
            let (marker, value_style) = if selected {
                ("→ ", Style::default().fg(theme.accent))
            } else {
                ("  ", Style::default().fg(theme.text))
            };

            let mut spans = vec![
                Span::styled(marker, Style::default().fg(theme.accent)),
                Span::styled(
                    candidate.value.clone(),
                    if selected {
                        value_style.add_modifier(Modifier::BOLD)
                    } else {
                        value_style
                    },
                ),
            ];
            if !candidate.detail.is_empty() {
                let gap = value_width - candidate.value.chars().count() + 2;
                spans.push(Span::styled(
                    format!("{}{}", " ".repeat(gap), candidate.detail),
                    Style::default().fg(theme.muted),
                ));
            }

            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let fill = usize::from(width).saturating_sub(used);
            spans.push(Span::raw(" ".repeat(fill)));
            Line::from(spans)
        })
        .collect()
}

/// Which slice of the candidate list is on screen, scrolled to keep the selection visible.
fn window(completion: &Completion) -> std::ops::Range<usize> {
    let total = completion.candidates.len();
    if total <= MAX_VISIBLE {
        return 0..total;
    }
    let start = completion
        .selected
        .saturating_sub(MAX_VISIBLE - 1)
        .min(total - MAX_VISIBLE);
    start..start + MAX_VISIBLE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_paths(_: &str) -> Vec<String> {
        Vec::new()
    }

    fn some_paths(_: &str) -> Vec<String> {
        ["src/main.rs", "src/lib.rs", "Cargo.toml"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn a_bare_slash_offers_every_command() {
        let c = resolve("/", 1, &no_paths).expect("completion");
        assert_eq!(c.kind, Kind::Command);
        assert_eq!(c.candidates.len(), commands().len());
    }

    #[test]
    fn typing_narrows_the_command_list() {
        let c = resolve("/qu", 3, &no_paths).expect("completion");
        assert_eq!(c.current().map(|c| c.value.as_str()), Some("/quit"));
    }

    #[test]
    fn a_slash_after_text_is_not_a_command() {
        assert!(resolve("say /quit", 9, &no_paths).is_none());
    }

    #[test]
    fn a_command_with_an_argument_closes_the_palette() {
        assert!(resolve("/model gpt", 10, &no_paths).is_none());
    }

    #[test]
    fn an_at_sign_completes_paths() {
        let c = resolve("look at @src", 12, &some_paths).expect("completion");
        assert_eq!(c.kind, Kind::Path);
        assert!(c.candidates.iter().any(|c| c.value == "src/main.rs"));
    }

    #[test]
    fn a_path_token_replaces_the_at_sign_too() {
        let c = resolve("look at @src", 12, &some_paths).expect("completion");
        assert_eq!(c.token_start, 8, "the @ is at index 8 and is replaced");
    }

    #[test]
    fn whitespace_after_an_at_sign_closes_the_popup() {
        assert!(resolve("@src and more", 13, &some_paths).is_none());
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut c = resolve("/", 1, &no_paths).expect("completion");
        let last = c.candidates.len() - 1;
        c.prev();
        assert_eq!(c.selected, last);
        c.next();
        assert_eq!(c.selected, 0);
    }

    #[test]
    fn the_window_scrolls_to_keep_the_selection_visible() {
        let candidates: Vec<Candidate> = (0..20)
            .map(|i| Candidate {
                value: format!("item{i}"),
                detail: String::new(),
            })
            .collect();
        let c = Completion {
            kind: Kind::Path,
            candidates,
            selected: 15,
            token_start: 0,
        };
        let w = window(&c);
        assert!(w.contains(&15), "{w:?}");
        assert_eq!(w.len(), MAX_VISIBLE);
    }

    #[test]
    fn every_overlay_row_fills_the_width() {
        let c = resolve("/", 1, &no_paths).expect("completion");
        for line in render(&c, 50, &Theme::default()) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(width, 50);
        }
    }
}
