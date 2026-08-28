//! A list you choose from.
//!
//! The completion popup ranks fragments of what you are typing; this presents a closed set and
//! asks which one. They look alike on purpose — same rows, same highlight, same corner of the
//! screen — because they are the same gesture, and a second visual language for "pick one"
//! would be a second thing to learn.
//!
//! Built for models, where the set is long, mostly unreachable, and the reason a given entry
//! is unreachable is the single most useful thing on the row.

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
    /// Everything on offer, unfiltered.
    ///
    /// Kept whole so backspacing widens the list again. Filtering destructively would mean a
    /// typo could only be recovered from by closing the list and opening it afresh.
    all: Vec<Choice>,
    /// What has been typed to narrow it.
    query: String,
    /// The rows currently on offer, in rank order.
    pub choices: Vec<Choice>,
    /// Which row is highlighted.
    pub selected: usize,
    /// Something to say about the last attempt, shown in the heading.
    ///
    /// Set when a row that cannot be taken is taken anyway. The list stays open: the answer to
    /// "that one needs a key" is to choose a different one, and closing the list means
    /// reopening it and retyping the query to do so.
    notice: Option<String>,
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
            // Failing that, the first row that can actually be taken. Row zero is where the
            // unusable ones tend to sort, and opening on one makes the list look inert.
            .or_else(|| choices.iter().position(|c| c.ready))
            .unwrap_or(0);
        Self {
            title: title.into(),
            all: choices.clone(),
            query: String::new(),
            choices,
            selected,
            notice: None,
        }
    }

    /// Try to take the highlighted row.
    ///
    /// `None` when it cannot be taken, in which case the list stays open and says why. Refused
    /// here rather than by the daemon because the row already carries the reason: a round trip
    /// to be told what is written on screen would close the list to say it.
    pub fn take(&mut self) -> Option<String> {
        let choice = self.current()?.clone();
        if choice.ready {
            return Some(choice.value);
        }
        self.notice = Some(format!("{} — {}", choice.value, choice.detail));
        None
    }

    /// What has been typed so far.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Narrow the list by one more character.
    ///
    /// Fuzzy, and against the value rather than the detail: somebody typing `sonnet` wants the
    /// models called that, not every row whose reason-it-is-unavailable happens to contain
    /// those letters.
    pub fn push(&mut self, c: char) {
        self.notice = None;
        self.query.push(c);
        self.refilter();
    }

    /// Widen it again by one.
    pub fn pop(&mut self) -> bool {
        self.notice = None;
        let popped = self.query.pop().is_some();
        if popped {
            self.refilter();
        }
        popped
    }

    /// Rebuild the offered rows from the query.
    fn refilter(&mut self) {
        if self.query.is_empty() {
            self.choices = self.all.clone();
        } else {
            let values: Vec<String> = self.all.iter().map(|c| c.value.clone()).collect();
            let ranked = crate::fuzzy::filter(&self.query, &values);
            self.choices = ranked
                .into_iter()
                .filter_map(|value| self.all.iter().find(|c| &c.value == value).cloned())
                .collect();
            // Readiness outranks the match score. Typing `gpt` used to put five rows needing
            // a key you have not set above the one you can actually run, and the highlight
            // landed on the first of them -- a list that answers a keystroke with a refusal.
            // Stable, so the fuzzy order survives within each group.
            self.choices.sort_by_key(|c| !c.ready);
        }
        // Onto something usable, and back to the top otherwise: the previous highlight was a
        // position in a different list, and keeping the index lands on whatever is there now.
        self.selected = self.choices.iter().position(|c| c.ready).unwrap_or(0);
    }

    /// Whether there is anything to choose right now.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.choices.is_empty()
    }

    /// Move the highlight down, wrapping.
    pub fn next(&mut self) {
        self.notice = None;
        if !self.choices.is_empty() {
            self.selected = (self.selected + 1) % self.choices.len();
        }
    }

    /// Move the highlight up, wrapping.
    pub fn previous(&mut self) {
        self.notice = None;
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

    /// Whether the list started with anything at all.
    ///
    /// Distinct from having nothing *right now*: a query that matches nothing is a list you
    /// can back out of, and a catalog with nothing in it is not.
    #[must_use]
    pub fn offers_nothing(&self) -> bool {
        self.all.is_empty()
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

mod view;

pub use view::render;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::text::Line;

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
        assert!(shown[0].contains("1 of 3"), "{:?}", shown[0]);
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

#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::theme::Theme;

    fn many() -> Vec<Choice> {
        [
            "openrouter/deepseek/deepseek-v3.2",
            "openrouter/anthropic/claude-sonnet-4.5",
            "anthropic/claude-haiku-4-5",
            "ollama/llama3.3",
        ]
        .iter()
        .map(|name| Choice {
            value: (*name).to_owned(),
            detail: "1k".into(),
            ready: true,
        })
        .collect()
    }

    fn values(picker: &Picker) -> Vec<&str> {
        picker.choices.iter().map(|c| c.value.as_str()).collect()
    }

    #[test]
    fn typing_narrows_the_list() {
        // Fifty-three rows is more than anyone should arrow through.
        let mut picker = Picker::new("Model", many(), None);
        for c in "sonnet".chars() {
            picker.push(c);
        }
        assert_eq!(
            values(&picker),
            vec!["openrouter/anthropic/claude-sonnet-4.5"]
        );
    }

    #[test]
    fn backspacing_widens_it_again() {
        // Destructive filtering would mean a typo could only be undone by closing the list.
        let mut picker = Picker::new("Model", many(), None);
        for c in "sonnetx".chars() {
            picker.push(c);
        }
        assert!(picker.is_empty(), "the typo matches nothing");
        picker.pop();
        assert_eq!(
            values(&picker),
            vec!["openrouter/anthropic/claude-sonnet-4.5"]
        );
    }

    #[test]
    fn clearing_the_query_offers_everything_again() {
        let mut picker = Picker::new("Model", many(), None);
        picker.push('z');
        while picker.pop() {}
        assert_eq!(picker.choices.len(), 4);
    }

    #[test]
    fn narrowing_puts_the_highlight_back_at_the_top() {
        // The old index was a position in a different list, and keeping it lands on whatever
        // happens to be there now.
        let mut picker = Picker::new("Model", many(), None);
        picker.next();
        picker.next();
        picker.push('l');
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn a_query_matching_nothing_says_so_rather_than_going_blank() {
        let mut picker = Picker::new("Model", many(), None);
        for c in "zzzz".chars() {
            picker.push(c);
        }
        let shown: Vec<String> = render(&picker, 60, &Theme::default())
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(shown[0].contains("nothing matches"), "{shown:?}");
        assert!(shown[0].contains("zzzz"), "{shown:?}");
    }

    #[test]
    fn a_query_matching_nothing_is_not_the_same_as_an_empty_catalog() {
        // One you can back out of; the other is a configuration with no providers in it.
        let mut picker = Picker::new("Model", many(), None);
        picker.push('z');
        assert!(picker.is_empty());
        assert!(!picker.offers_nothing());
        assert!(Picker::new("Model", Vec::new(), None).offers_nothing());
    }

    #[test]
    fn the_heading_shows_what_was_typed() {
        let mut picker = Picker::new("Model", many(), None);
        picker.push('l');
        let shown: Vec<String> = render(&picker, 60, &Theme::default())
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(shown[0].contains("▸ l"), "{:?}", shown[0]);
    }
}

#[cfg(test)]
mod take_tests {
    use super::*;
    use crate::theme::Theme;

    fn mixed() -> Vec<Choice> {
        vec![
            Choice {
                value: "ready/one".into(),
                detail: "1k".into(),
                ready: true,
            },
            Choice {
                value: "locked/two".into(),
                detail: "set TWO_KEY".into(),
                ready: false,
            },
        ]
    }

    #[test]
    fn a_usable_row_is_taken() {
        let mut picker = Picker::new("Model", mixed(), None);
        assert_eq!(picker.take(), Some("ready/one".to_owned()));
    }

    #[test]
    fn a_locked_row_is_refused_without_closing_anything() {
        // Picking it can only fail, and the failure is written on the row. A round trip to be
        // told that would close the list to say it, and then you retype the query.
        let mut picker = Picker::new("Model", mixed(), None);
        picker.next();
        assert_eq!(picker.take(), None);
        let heading: String = render(&picker, 70, &Theme::default())[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(heading.contains("set TWO_KEY"), "{heading}");
    }

    #[test]
    fn moving_on_clears_what_was_said_about_the_last_one() {
        let mut picker = Picker::new("Model", mixed(), None);
        picker.next();
        let _ = picker.take();
        picker.previous();
        let heading: String = render(&picker, 70, &Theme::default())[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!heading.contains("TWO_KEY"), "{heading}");
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    fn mixed() -> Vec<Choice> {
        vec![
            Choice {
                value: "openai/gpt-5".into(),
                detail: "set OPENAI_API_KEY".into(),
                ready: false,
            },
            Choice {
                value: "openai/gpt-5.1".into(),
                detail: "set OPENAI_API_KEY".into(),
                ready: false,
            },
            Choice {
                value: "openrouter/openai/gpt-5.1".into(),
                detail: "400k".into(),
                ready: true,
            },
            Choice {
                value: "local/llama".into(),
                detail: "131k".into(),
                ready: true,
            },
        ]
    }

    #[test]
    fn a_filter_puts_what_you_can_run_first() {
        // Typing `gpt` used to rank five rows needing an unset key above the one usable match.
        let mut picker = Picker::new("Model", mixed(), None);
        for c in "gpt".chars() {
            picker.push(c);
        }
        assert!(
            picker.current().expect("a row").ready,
            "the top row is usable"
        );
        assert_eq!(
            picker.current().expect("a row").value,
            "openrouter/openai/gpt-5.1"
        );
    }

    #[test]
    fn the_highlight_lands_somewhere_enter_will_work() {
        let mut picker = Picker::new("Model", mixed(), None);
        for c in "gpt".chars() {
            picker.push(c);
        }
        assert_eq!(picker.take(), Some("openrouter/openai/gpt-5.1".to_owned()));
    }

    #[test]
    fn opening_with_no_current_value_skips_the_unusable() {
        let picker = Picker::new("Model", mixed(), None);
        assert!(
            picker.current().expect("a row").ready,
            "not opened on a refusal"
        );
    }

    #[test]
    fn a_current_value_still_wins_over_readiness() {
        // Where you already are outranks the rule: you opened the list to move from it.
        let picker = Picker::new("Model", mixed(), Some("openai/gpt-5.1"));
        assert_eq!(picker.current().expect("a row").value, "openai/gpt-5.1");
    }

    #[test]
    fn a_filter_matching_nothing_usable_still_shows_the_reasons() {
        let none_ready = vec![
            Choice {
                value: "openai/gpt-5".into(),
                detail: "set OPENAI_API_KEY".into(),
                ready: false,
            },
            Choice {
                value: "azure/gpt-5".into(),
                detail: "set AZURE_OPENAI_API_KEY".into(),
                ready: false,
            },
        ];
        let mut picker = Picker::new("Model", none_ready, None);
        picker.push('g');
        assert!(!picker.is_empty(), "the rows are still listed");
        assert!(picker.take().is_none(), "and taking one says why instead");
        assert!(picker.notice.is_some());
    }
}

#[cfg(test)]
mod fluidity_tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::text::Line;

    fn many(n: usize) -> Vec<Choice> {
        (1..=n)
            .map(|i| Choice {
                value: format!("provider/model-{i}"),
                detail: "131k".into(),
                ready: true,
            })
            .collect()
    }

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_long_list_says_how_much_is_below() {
        // Fifty rows in a window of eight gave no sign there was anything past the eighth, so
        // moving through it felt like the list was changing under you.
        let picker = Picker::new("Model", many(50), None);
        let shown = text(&render(&picker, 60, &Theme::default()));
        assert!(shown[0].contains('↓'), "{:?}", shown[0]);
        assert!(
            !shown[0].contains('↑'),
            "nothing is above the top: {:?}",
            shown[0]
        );
    }

    #[test]
    fn scrolling_down_says_how_much_is_above() {
        let mut picker = Picker::new("Model", many(50), None);
        for _ in 0..40 {
            picker.next();
        }
        let shown = text(&render(&picker, 60, &Theme::default()));
        assert!(shown[0].contains('↑'), "{:?}", shown[0]);
    }

    #[test]
    fn a_list_that_fits_says_neither() {
        let picker = Picker::new("Model", many(3), None);
        let shown = text(&render(&picker, 60, &Theme::default()));
        assert!(
            !shown[0].contains('↑') && !shown[0].contains('↓'),
            "{:?}",
            shown[0]
        );
    }

    #[test]
    fn the_highlight_bar_reaches_the_edge() {
        // A background that stops where the text stops reads as a ragged block, which is worse
        // than no highlight at all.
        let picker = Picker::new("Model", many(5), None);
        let rendered = render(&picker, 60, &Theme::default());
        let row = &rendered[1];
        let painted: usize = row.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(painted, 60, "the selected row fills the width");
        assert!(
            row.spans.iter().any(|s| s.style.bg.is_some()),
            "and it is filled"
        );
    }

    #[test]
    fn an_unselected_row_is_the_block_colour_not_the_bar_colour() {
        // Every row carries a background now — that is what makes the list read as one object
        // rather than as loose text. What marks the selection is that its background differs.
        let theme = Theme::default();
        let picker = Picker::new("Model", many(5), None);
        let rendered = render(&picker, 60, &theme);
        assert!(
            rendered[2]
                .spans
                .iter()
                .all(|s| s.style.bg == Some(theme.menu_bg)),
            "the block"
        );
        assert!(
            rendered[1]
                .spans
                .iter()
                .any(|s| s.style.bg == Some(theme.menu_sel_bg)),
            "and the bar on the row above it"
        );
    }
}
