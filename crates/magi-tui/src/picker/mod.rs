//! Selectable, filterable choices with optional detail rows.

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub detail: String,
    pub ready: bool,
}

/// An open list.
#[derive(Debug, Clone)]
pub struct Picker {
    presentation: Option<Box<table::Presentation>>,
    detail_below: bool,
    pub title: String,
    all: Vec<Choice>,
    query: String,
    pub choices: Vec<Choice>,
    pub selected: usize,
    notice: Option<String>,
    about: Vec<String>,
    painted: Vec<ratatui::text::Line<'static>>,
    asks: Option<String>,
    confirm: Option<Confirm>,
}

/// A question standing between a row and its removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub value: String,
    pub question: String,
    pub yes: bool,
}

impl Picker {
    /// Open a list, starting on `current` if it is in it.
    #[must_use]
    pub fn new(title: impl Into<String>, choices: Vec<Choice>, current: Option<&str>) -> Self {
        let selected = current
            .and_then(|name| choices.iter().position(|c| c.value == name))
            .or_else(|| choices.iter().position(|c| c.ready))
            .unwrap_or(0);
        Self {
            presentation: None,
            detail_below: false,
            title: title.into(),
            all: choices.clone(),
            query: String::new(),
            choices,
            selected,
            notice: None,
            about: Vec::new(),
            painted: Vec::new(),
            confirm: None,
            asks: None,
        }
    }

    /// Enable row removal with a confirmation question.
    #[must_use]
    pub fn askable(mut self, question: impl Into<String>) -> Self {
        self.asks = Some(question.into());
        self
    }

    /// What is being asked, if anything.
    #[must_use]
    pub fn asking(&self) -> Option<&Confirm> {
        self.confirm.as_ref()
    }

    /// Ask for confirmation about the highlighted row, starting with no selected.
    pub fn ask(&mut self) -> bool {
        let Some(question) = self.asks.clone() else {
            return false;
        };
        let Some(choice) = self.current().cloned() else {
            return false;
        };
        self.notice = None;
        self.confirm = Some(Confirm {
            question: question.replace("{}", &choice.value),
            value: choice.value,
            yes: false,
        });
        true
    }

    /// Put the highlight on the other answer.
    pub fn swap(&mut self) {
        if let Some(asking) = self.confirm.as_mut() {
            asking.yes = !asking.yes;
        }
    }

    /// Close the confirmation and return its value only for a yes answer.
    pub fn answer(&mut self, yes: bool) -> Option<String> {
        let asking = self.confirm.take()?;
        yes.then_some(asking.value)
    }

    /// Set plain lines above the choices.
    #[must_use]
    pub fn about(mut self, rows: Vec<String>) -> Self {
        self.about = rows;
        self
    }

    /// Set styled lines above the choices.
    #[must_use]
    pub fn painted(mut self, rows: Vec<ratatui::text::Line<'static>>) -> Self {
        self.painted = rows;
        self
    }

    /// What is drawn under the heading, painted where the caller painted it.
    #[must_use]
    pub fn drawn_about(&self) -> Vec<ratatui::text::Line<'static>> {
        if !self.painted.is_empty() {
            return self.painted.clone();
        }
        self.about
            .iter()
            .map(|row| {
                ratatui::text::Line::from(ratatui::text::Span::styled(
                    row.clone(),
                    ratatui::style::Style::default().fg(crate::colour::muted()),
                ))
            })
            .collect()
    }

    /// What this list is deciding about.
    #[must_use]
    pub fn asking_about(&self) -> &[String] {
        &self.about
    }

    /// Try to take the highlighted row.
    pub fn take(&mut self) -> Option<String> {
        let choice = self.current()?.clone();
        if choice.ready {
            return Some(choice.value);
        }
        let reason = self
            .reason(&choice.value)
            .unwrap_or(choice.detail.as_str())
            .to_owned();
        self.notice = Some(format!("{} — {reason}", choice.value));
        None
    }

    /// Try to take the choice at `index` directly (for numbered decision prompts).
    pub fn take_at(&mut self, index: usize) -> Option<String> {
        if index >= self.choices.len() {
            return None;
        }
        self.selected = index;
        self.take()
    }

    /// What has been typed so far.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Narrow the list by one more character.
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
            self.choices.sort_by_key(|c| !c.ready);
        }
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

    /// Show the highlighted value's display text above the rows without changing its identity.
    pub fn captions(&mut self, captions: impl IntoIterator<Item = (String, String)>) {
        self.presentation.get_or_insert_default().captions = captions
            .into_iter()
            .map(|(value, caption)| {
                (
                    value,
                    caption
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(512)
                        .collect(),
                )
            })
            .collect();
    }

    fn caption(&self) -> Option<&str> {
        self.presentation
            .as_deref()?
            .captions
            .get(&self.current()?.value)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Record refusal reasons for unready choices when details hold table cells.
    pub fn reasons(&mut self, reasons: impl IntoIterator<Item = (String, String)>) {
        self.presentation.get_or_insert_default().reasons = reasons
            .into_iter()
            .filter(|(_, reason)| !reason.is_empty())
            .collect();
    }

    fn reason(&self, value: &str) -> Option<&str> {
        self.presentation
            .as_deref()?
            .reasons
            .get(value)
            .map(String::as_str)
            .filter(|reason| !reason.is_empty())
    }

    /// Show selectable scope tabs above the rows.
    pub fn tabs(&mut self, tabs: Vec<String>, active: usize) {
        let presentation = self.presentation.get_or_insert_default();
        presentation.tab = active.min(tabs.len().saturating_sub(1));
        presentation.tabs = tabs;
    }

    #[must_use]
    pub fn has_tabs(&self) -> bool {
        self.tab_labels().len() > 1
    }

    pub(super) fn tab_labels(&self) -> &[String] {
        self.presentation
            .as_deref()
            .map(|presentation| presentation.tabs.as_slice())
            .unwrap_or_default()
    }

    pub(super) fn active_tab(&self) -> usize {
        self.presentation
            .as_deref()
            .map_or(0, |presentation| presentation.tab)
    }

    /// Show a concise control hint in the heading.
    pub fn hint(&mut self, hint: impl Into<String>) {
        let hint = hint.into();
        self.presentation.get_or_insert_default().hint = (!hint.is_empty()).then_some(hint);
    }

    pub(super) fn hint_text(&self) -> Option<&str> {
        self.presentation
            .as_deref()?
            .hint
            .as_deref()
            .filter(|hint| !hint.is_empty())
    }

    /// Present this picker as a structured decision prompt rather than a searchable list.
    pub fn decision(&mut self, decision: Decision) {
        self.presentation.get_or_insert_default().decision = Some(decision);
    }

    #[must_use]
    pub fn quick_keys(&self) -> bool {
        self.decision_config()
            .is_some_and(|decision| decision.quick_keys)
    }

    pub(super) fn decision_config(&self) -> Option<&Decision> {
        self.presentation.as_deref()?.decision.as_ref()
    }

    /// Rows this needs on screen, heading included.
    #[must_use]
    pub fn height(&self) -> u16 {
        let tabs = usize::from(self.has_tabs());
        if self.choices.is_empty() {
            return u16::try_from(1 + tabs).unwrap_or(u16::MAX);
        }
        let rows: usize = self
            .choices
            .get(self.window())
            .unwrap_or_default()
            .iter()
            .map(|choice| self.choice_height(choice))
            .sum();
        u16::try_from(
            rows + 1
                + tabs
                + usize::from(!self.columns().is_empty())
                + usize::from(self.caption().is_some()),
        )
        .unwrap_or(u16::MAX)
    }

    /// Align tab-separated detail cells under column headings.
    pub fn table(&mut self, columns: &[&str]) {
        self.presentation.get_or_insert_default().headers =
            columns.iter().map(|column| (*column).to_owned()).collect();
        self.detail_below = false;
    }

    fn columns(&self) -> &[String] {
        self.presentation
            .as_deref()
            .map(|columns| columns.headers.as_slice())
            .unwrap_or_default()
    }

    /// Put each line of a choice's detail below its name.
    pub fn details_below(&mut self) {
        self.detail_below = true;
    }

    /// Whether the unfiltered list is empty.
    #[must_use]
    pub fn offers_nothing(&self) -> bool {
        self.all.is_empty()
    }

    /// Which slice is on screen, scrolled to keep the highlight visible.
    fn window(&self) -> std::ops::Range<usize> {
        let visible = if self.detail_below {
            let height = self
                .choices
                .iter()
                .map(|choice| self.choice_height(choice))
                .max()
                .unwrap_or(2);
            (max_visible() / height).max(1)
        } else {
            max_visible()
        };
        let total = self.choices.len();
        if total <= visible {
            return 0..total;
        }
        let start = self
            .selected
            .saturating_sub(visible - 1)
            .min(total - visible);
        start..start + visible
    }

    fn choice_height(&self, choice: &Choice) -> usize {
        if self.detail_below {
            1 + choice.detail.split('\n').count()
        } else {
            1
        }
    }
}

mod decision;
mod table;
mod view;

pub use decision::{Advice, Decision, Subject, Tone};
pub use view::{render, render_in};

/// Rows shown at once, as `magi.ui.menu_rows` left it.
fn max_visible() -> usize {
    usize::from(crate::metric::menu_rows())
}

#[cfg(test)]
mod behaving;

#[cfg(test)]
mod tests;
