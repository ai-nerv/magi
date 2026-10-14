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
    columns: Option<Box<table::Columns>>,
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
            columns: None,
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
        self.notice = Some(format!("{} — {}", choice.value, choice.detail));
        None
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

    /// Rows this needs on screen, heading included.
    #[must_use]
    pub fn height(&self) -> u16 {
        let rows: usize = self
            .choices
            .get(self.window())
            .unwrap_or_default()
            .iter()
            .map(|choice| self.choice_height(choice))
            .sum();
        u16::try_from(
            rows + 1 + usize::from(!self.columns().is_empty() && !self.choices.is_empty()),
        )
        .unwrap_or(u16::MAX)
    }

    /// Align tab-separated detail cells under column headings.
    pub fn table(&mut self, columns: &[&str]) {
        self.columns = Some(Box::new(table::Columns {
            headers: columns.iter().map(|column| (*column).to_owned()).collect(),
        }));
        self.detail_below = false;
    }

    fn columns(&self) -> &[String] {
        self.columns
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

mod table;
mod view;

pub use view::render;

/// Rows shown at once, as `magi.ui.menu_rows` left it.
fn max_visible() -> usize {
    usize::from(crate::metric::menu_rows())
}

#[cfg(test)]
mod behaving;

#[cfg(test)]
mod tests;
