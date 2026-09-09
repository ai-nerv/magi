//! Prompt autocompletion. Two triggers, as in Pi: `/` at the start of the prompt opens the command
//! palette, `@` anywhere completes a path. Driven from the editor's line, holding no state it owns.

use crate::fuzzy;
use ratatui::text::Line;

/// What is being completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Command,
    Path,
    Instance,
    Skill,
}

/// One offered completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub detail: String,
}

/// An open completion popup.
#[derive(Debug, Clone)]
pub struct Completion {
    pub kind: Kind,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    /// What the user typed, derived here so it cannot drift from what the candidates were ranked
    /// against. The renderer picks those characters out of each candidate.
    pub typed: String,
    /// Character index in the line where the replaced token starts.
    pub token_start: usize,
}

impl Completion {
    #[must_use]
    pub fn current(&self) -> Option<&Candidate> {
        self.candidates.get(self.selected)
    }

    pub fn next(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected + 1) % self.candidates.len();
        }
    }

    pub fn prev(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.candidates.len() - 1);
        }
    }

    #[must_use]
    pub fn height(&self) -> u16 {
        self.candidates.len().min(max_visible()) as u16
    }
}

/// The commands M0 can honour. Colon, not slash: `/` is search, of the transcript now and of
/// balthasar's memory later, and a prefix cannot mean both.
#[must_use]
pub fn commands() -> Vec<Candidate> {
    [
        (":help", "show keybindings and commands"),
        (":clear", "start a fresh conversation"),
        (":model", "the model, or :model <name> to switch"),
        (":permissions", "ask the model what it needs, and decide"),
        (":resume", "continue a session from this directory"),
        (":trace", "what this session has done, as it happened"),
        (":rewind", "undo the last exchange, or :rewind N"),
        (":think", "how much reasoning to ask for"),
        (":quit", "exit magi, and :q for the same"),
        (
            ":quitall",
            "exit, taking anything this session started — :qa for the same",
        ),
    ]
    .iter()
    .map(|(value, detail)| Candidate {
        value: (*value).to_owned(),
        detail: (*detail).to_owned(),
    })
    .collect()
}

/// Work out what, if anything, should be completed for `line` with the cursor at `col`. `list_paths`
/// is a parameter so this stays pure and the filesystem lives in the caller.
pub fn resolve(
    line: &str,
    col: usize,
    list_paths: &dyn Fn(&str) -> Vec<String>,
) -> Option<Completion> {
    resolve_with(line, col, list_paths, &|_| Vec::new())
}

/// The same, told where instance names come from. Two entry points because the filesystem and the
/// list of running siblings are found by different callers.
pub fn resolve_with(
    line: &str,
    col: usize,
    list_paths: &dyn Fn(&str) -> Vec<String>,
    list_instances: &dyn Fn(&str) -> Vec<String>,
) -> Option<Completion> {
    let before: String = line.chars().take(col).collect();
    let token = crate::trigger::under(&before, &crate::trigger::EVERY)?;

    // Which of the four it is decides where the candidates come from, and nothing else. Some
    // candidates carry their sigil and some do not -- `:help` is written with the colon and
    // `src/main.rs` is not -- and the needle has to be the same shape as what it is matched against.
    let (kind, offered, sigil_in_value) = match token.trigger {
        crate::trigger::Trigger::Command => (Kind::Command, commands(), true),
        crate::trigger::Trigger::File => (
            Kind::Path,
            list_paths(&token.query)
                .into_iter()
                .map(|value| Candidate {
                    value,
                    detail: String::new(),
                })
                .collect(),
            false,
        ),
        crate::trigger::Trigger::Instance => (
            Kind::Instance,
            list_instances(&token.query)
                .into_iter()
                .map(|value| Candidate {
                    value: format!("${value}"),
                    detail: String::new(),
                })
                .collect(),
            true,
        ),
        // Skills are not built yet; an empty list means no popup.
        crate::trigger::Trigger::Skill => (Kind::Skill, Vec::new(), true),
    };

    let values: Vec<String> = offered.iter().map(|c| c.value.clone()).collect();
    let needle = if sigil_in_value {
        token.written()
    } else {
        token.query.clone()
    };
    let ranked = fuzzy::filter(&needle, &values);
    let candidates: Vec<Candidate> = ranked
        .into_iter()
        .take(max_visible() * 4)
        .filter_map(|value| offered.iter().find(|c| c.value == *value).cloned())
        .collect();

    (!candidates.is_empty()).then_some(Completion {
        kind,
        typed: token.written(),
        candidates,
        selected: 0,
        token_start: token.at,
    })
}

#[must_use]
pub fn render(completion: &Completion, width: u16) -> Vec<Line<'static>> {
    let window = window(completion);
    // Details line up in a column: a ragged left edge reads as noise rather than as choices.
    let value_width = completion.candidates[window.clone()]
        .iter()
        .map(|c| c.value.chars().count())
        .max()
        .unwrap_or(0);
    // So the part of each candidate already typed can be told apart from the part it adds.
    let typed = completion.typed.clone();

    completion.candidates[window.clone()]
        .iter()
        .enumerate()
        .map(|(offset, candidate)| {
            crate::menu::row(
                &crate::menu::Row {
                    value: &candidate.value,
                    detail: &candidate.detail,
                    selected: window.start + offset == completion.selected,
                    ready: true,
                    value_width,
                },
                &typed,
                width,
            )
        })
        .collect()
}

/// Which slice of the candidate list is on screen, scrolled to keep the selection visible.
fn window(completion: &Completion) -> std::ops::Range<usize> {
    let total = completion.candidates.len();
    if total <= max_visible() {
        return 0..total;
    }
    let start = completion
        .selected
        .saturating_sub(max_visible() - 1)
        .min(total - max_visible());
    start..start + max_visible()
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
    fn a_bare_colon_offers_every_command() {
        let c = resolve(":", 1, &no_paths).expect("completion");
        assert_eq!(c.kind, Kind::Command);
        assert_eq!(c.candidates.len(), commands().len());
    }

    #[test]
    fn typing_narrows_the_command_list() {
        let c = resolve(":qu", 3, &no_paths).expect("completion");
        assert_eq!(c.current().map(|c| c.value.as_str()), Some(":quit"));
    }

    #[test]
    fn a_slash_after_text_is_not_a_command() {
        assert!(resolve("say /quit", 9, &no_paths).is_none());
    }

    #[test]
    fn a_command_with_an_argument_closes_the_palette() {
        assert!(resolve(":model gpt", 10, &no_paths).is_none());
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
        let mut c = resolve(":", 1, &no_paths).expect("completion");
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
            typed: String::new(),
            candidates,
            selected: 15,
            token_start: 0,
        };
        let w = window(&c);
        assert!(w.contains(&15), "{w:?}");
        assert_eq!(w.len(), max_visible());
    }

    #[test]
    fn every_overlay_row_fills_the_width() {
        let c = resolve(":", 1, &no_paths).expect("completion");
        for line in render(&c, 50) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(width, 50);
        }
    }
}

#[cfg(test)]
mod clip_tests {
    use super::*;

    #[test]
    fn a_detail_longer_than_the_popup_is_cut_rather_than_overflowing() {
        // A row wider than the popup does not wrap: it pushes the layout sideways.
        let completion = Completion {
            kind: Kind::Command,
            typed: String::new(),
            candidates: vec![Candidate {
                value: ":x".to_owned(),
                detail: "a description far longer than the space available for it".to_owned(),
            }],
            selected: 0,
            token_start: 0,
        };
        for line in render(&completion, 30) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(width, 30, "{line:?}");
        }
    }

    #[test]
    fn a_popup_too_narrow_for_any_detail_still_renders() {
        let completion = Completion {
            kind: Kind::Command,
            typed: String::new(),
            candidates: vec![Candidate {
                value: ":x".to_owned(),
                detail: "anything".to_owned(),
            }],
            selected: 0,
            token_start: 0,
        };
        for line in render(&completion, 4) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(width <= 4, "{width}");
        }
    }
}

/// Rows shown at once, as `magi.ui.menu_rows` left it.
fn max_visible() -> usize {
    usize::from(crate::metric::menu_rows())
}

#[must_use]
pub fn rows() -> usize {
    max_visible()
}
