//! A list you choose from.
//!
//! The completion popup ranks fragments of what you are typing; this presents a closed set and
//! asks which one. They look alike on purpose — same rows, same highlight, same corner of the
//! screen — because they are the same gesture, and a second visual language for "pick one"
//! would be a second thing to learn.
//!
//! Built for models, where the set is long, mostly unreachable, and the reason a given entry
//! is unreachable is the single most useful thing on the row.

use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Rows shown at once. The same budget the completion popup uses.
const MAX_VISIBLE: usize = 8;

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What is sent when this is picked.
    pub value: String,
    /// Shown after the value, dimmed.
    pub detail: String,
    /// Whether picking it would work.
    ///
    /// Unready rows stay in the list rather than being filtered out: somebody who has
    /// configured nothing would otherwise be shown an empty list, when what they need is the
    /// name of the variable to set.
    pub ready: bool,
}

/// An open list.
#[derive(Debug, Clone)]
pub struct Picker {
    /// What is being chosen, shown as a heading.
    pub title: String,
    /// Every option, in the order they are offered.
    pub choices: Vec<Choice>,
    /// Which row is highlighted.
    pub selected: usize,
}

impl Picker {
    /// Open a list, starting on `current` if it is in it.
    ///
    /// Starting on the current value rather than at the top: a list of forty opened at row one
    /// makes you find where you already are before you can move from it.
    #[must_use]
    pub fn new(title: impl Into<String>, choices: Vec<Choice>, current: Option<&str>) -> Self {
        let selected = current
            .and_then(|name| choices.iter().position(|c| c.value == name))
            .unwrap_or(0);
        Self {
            title: title.into(),
            choices,
            selected,
        }
    }

    /// Whether there is anything to choose.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.choices.is_empty()
    }

    /// Move the highlight down, wrapping.
    pub fn next(&mut self) {
        if !self.choices.is_empty() {
            self.selected = (self.selected + 1) % self.choices.len();
        }
    }

    /// Move the highlight up, wrapping.
    pub fn previous(&mut self) {
        if !self.choices.is_empty() {
            self.selected = (self.selected + self.choices.len() - 1) % self.choices.len();
        }
    }

    /// The highlighted row.
    #[must_use]
    pub fn current(&self) -> Option<&Choice> {
        self.choices.get(self.selected)
    }

    /// Rows this needs on screen, heading included.
    #[must_use]
    pub fn height(&self) -> u16 {
        u16::try_from(self.choices.len().min(MAX_VISIBLE) + 1).unwrap_or(u16::MAX)
    }

    /// Which slice is on screen, scrolled to keep the highlight visible.
    fn window(&self) -> std::ops::Range<usize> {
        let total = self.choices.len();
        if total <= MAX_VISIBLE {
            return 0..total;
        }
        let start = self
            .selected
            .saturating_sub(MAX_VISIBLE - 1)
            .min(total - MAX_VISIBLE);
        start..start + MAX_VISIBLE
    }
}

/// Draw the list.
#[must_use]
pub fn render(picker: &Picker, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let window = picker.window();
    let value_width = picker.choices[window.clone()]
        .iter()
        .map(|c| c.value.chars().count())
        .max()
        .unwrap_or(0);

    // A heading, because unlike the completion popup this is not obviously about what you just
    // typed: it appears because you asked a question, and it should say which one.
    let position = format!(" {}/{}", picker.selected + 1, picker.choices.len());
    let mut out = vec![Line::from(crate::fit(
        vec![
            Span::styled(
                picker.title.clone(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(position, Style::default().fg(theme.dim)),
        ],
        usize::from(width),
    ))];

    out.extend(
        picker.choices[window.clone()]
            .iter()
            .enumerate()
            .map(|(offset, choice)| {
                let selected = window.start + offset == picker.selected;
                let (marker, value_style) = if selected {
                    ("→ ", Style::default().fg(theme.accent))
                } else if choice.ready {
                    ("  ", Style::default().fg(theme.text))
                } else {
                    // Dimmed rather than hidden: it is real, it is just not ready.
                    ("  ", Style::default().fg(theme.dim))
                };

                let gap = value_width - choice.value.chars().count() + 2;
                let detail_style = if choice.ready {
                    Style::default().fg(theme.muted)
                } else {
                    Style::default().fg(theme.warning)
                };
                let spans = vec![
                    Span::styled(marker, Style::default().fg(theme.accent)),
                    Span::styled(
                        choice.value.clone(),
                        if selected {
                            value_style.add_modifier(Modifier::BOLD)
                        } else {
                            value_style
                        },
                    ),
                    Span::styled(
                        format!("{}{}", " ".repeat(gap), choice.detail),
                        detail_style,
                    ),
                ];
                Line::from(crate::fit(spans, usize::from(width)))
            }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choices() -> Vec<Choice> {
        vec![
            Choice {
                value: "a/one".into(),
                detail: "131k".into(),
                ready: true,
            },
            Choice {
                value: "b/two".into(),
                detail: "set B_KEY".into(),
                ready: false,
            },
            Choice {
                value: "c/three".into(),
                detail: "200k".into(),
                ready: true,
            },
        ]
    }

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn it_opens_on_what_is_already_chosen() {
        // A list of forty opened at row one makes you find where you are before you can move.
        let picker = Picker::new("Model", choices(), Some("c/three"));
        assert_eq!(picker.current().expect("a row").value, "c/three");
    }

    #[test]
    fn it_opens_at_the_top_when_nothing_is_chosen() {
        let picker = Picker::new("Model", choices(), None);
        assert_eq!(picker.current().expect("a row").value, "a/one");
    }

    #[test]
    fn it_opens_at_the_top_when_the_choice_is_not_in_the_list() {
        // A model set in config that the catalog no longer has, which is exactly the state
        // somebody is in when they come looking for this list.
        let picker = Picker::new("Model", choices(), Some("gone/away"));
        assert_eq!(picker.current().expect("a row").value, "a/one");
    }

    #[test]
    fn the_highlight_wraps_both_ways() {
        let mut picker = Picker::new("Model", choices(), None);
        picker.previous();
        assert_eq!(picker.current().expect("a row").value, "c/three");
        picker.next();
        assert_eq!(picker.current().expect("a row").value, "a/one");
    }

    #[test]
    fn an_unusable_choice_is_shown_with_what_it_needs() {
        // Filtering it out leaves an empty list for somebody who has configured nothing, and
        // an empty list does not tell them which variable to set.
        let shown = text(&render(
            &Picker::new("Model", choices(), None),
            60,
            &Theme::default(),
        ));
        assert!(shown.iter().any(|l| l.contains("b/two")), "{shown:?}");
        assert!(shown.iter().any(|l| l.contains("set B_KEY")), "{shown:?}");
    }

    #[test]
    fn the_heading_says_where_you_are_in_the_list() {
        let shown = text(&render(
            &Picker::new("Model", choices(), None),
            60,
            &Theme::default(),
        ));
        assert!(shown[0].contains("Model"), "{:?}", shown[0]);
        assert!(shown[0].contains("1/3"), "{:?}", shown[0]);
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_highlight_on_screen() {
        let many: Vec<Choice> = (0..30)
            .map(|i| Choice {
                value: format!("m/{i:02}"),
                detail: "1k".into(),
                ready: true,
            })
            .collect();
        let mut picker = Picker::new("Model", many, None);
        for _ in 0..20 {
            picker.next();
        }
        let shown = text(&render(&picker, 60, &Theme::default()));
        assert!(shown.iter().any(|l| l.contains("m/20")), "{shown:?}");
        assert_eq!(shown.len(), MAX_VISIBLE + 1, "heading plus a window");
    }

    #[test]
    fn every_row_fills_the_width() {
        let picker = Picker::new("Model", choices(), None);
        for line in render(&picker, 40, &Theme::default()) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(width, 40, "{line:?}");
        }
    }
}
