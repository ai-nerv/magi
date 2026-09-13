//! The info pane: what magi knows, shown in the middle, on request.
//!
//! One primitive, many views — a view is a function returning `Vec<Line>` plus a line in the
//! command table. Distinct from the menu slot, which holds things asking for the keyboard and is
//! reachable by tools over `casper surface`; nothing outside this process reaches the pane. It
//! draws over the transcript rather than reflowing it.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The share of the screen a float takes, in percent: width, then height.
const WIDTH: u16 = 80;
const HEIGHT: u16 = 70;

/// Rows the frame draws above the content (border, heading, blank) — taken off a click's row first.
const HEAD: u16 = 3;

/// A view drawn in the middle of the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub title: String,
    pub rows: Vec<Line<'static>>,
    pub top: usize,
    /// Show the newest end rather than the oldest. A flag, not a position: only the renderer knows
    /// how tall the panel is.
    pub follow: bool,
    /// What to say when there is nothing to show.
    pub empty: String,
    /// Parallel to `rows`: what a click on each row selects; empty for a view with no targets.
    pub picks: Vec<Option<String>>,
    /// The row the pointer is over, as an index into `rows`, lit while it is a selectable one.
    pub hover: Option<usize>,
}

impl Pane {
    /// A float titled `title`, holding `rows`.
    #[must_use]
    pub fn new(title: impl Into<String>, rows: Vec<Line<'static>>) -> Self {
        Self {
            title: title.into(),
            rows,
            top: 0,
            follow: false,
            empty: "nothing yet".to_owned(),
            picks: Vec::new(),
            hover: None,
        }
    }

    #[must_use]
    pub fn saying(mut self, empty: impl Into<String>) -> Self {
        self.empty = empty.into();
        self
    }

    /// Make rows selectable: `picks` runs parallel to `rows`, naming what each click attaches to.
    #[must_use]
    pub fn selectable(mut self, picks: Vec<Option<String>>) -> Self {
        self.picks = picks;
        self
    }

    /// What a click `row` cells below the panel's top edge selects, through `HEAD` and the scroll.
    #[must_use]
    pub fn selected(&self, row_in_panel: u16) -> Option<&str> {
        let content = usize::from(row_in_panel.checked_sub(HEAD)?);
        self.picks.get(self.top + content)?.as_deref()
    }

    /// Light the row `row_in_panel` cells below the top edge, if it is a selectable one, and say
    /// whether that changed anything — so a move within the same row skips a redraw.
    pub fn hover_at(&mut self, row_in_panel: u16) -> bool {
        let was = self.hover;
        self.hover = row_in_panel
            .checked_sub(HEAD)
            .map(|content| self.top + usize::from(content))
            .filter(|at| self.picks.get(*at).is_some_and(Option::is_some));
        was != self.hover
    }

    #[must_use]
    pub fn following(mut self) -> Self {
        self.follow = true;
        self
    }

    /// Settle the scroll position for a viewport `page` tall. Clears [`Pane::follow`], so a later
    /// scroll up is not undone on the next draw.
    pub fn settle(&mut self, page: usize) {
        if self.follow {
            self.bottom(page);
            self.follow = false;
        }
    }

    /// Where it sits, given the whole screen. Centred, so an odd number of spare columns leans left.
    #[must_use]
    pub fn area(screen: Rect) -> Rect {
        let width = (screen.width * WIDTH / 100).clamp(20.min(screen.width), screen.width);
        let height = (screen.height * HEIGHT / 100).clamp(3.min(screen.height), screen.height);
        Rect {
            x: screen.x + (screen.width.saturating_sub(width)) / 2,
            y: screen.y + (screen.height.saturating_sub(height)) / 2,
            width,
            height,
        }
    }

    /// How many rows of content fit, given the whole screen. Four go elsewhere: two to the border,
    /// two to the heading and the blank line under it.
    #[must_use]
    pub fn page(screen: Rect) -> usize {
        Self::page_of(Self::area(screen))
    }

    /// The same, for a caller that already holds the panel's own rectangle.
    #[must_use]
    pub fn page_of(area: Rect) -> usize {
        area.height.saturating_sub(4) as usize
    }

    /// The heading drawn inside the panel: what this is, and where in it you are.
    #[must_use]
    pub fn heading(&self, page: usize) -> String {
        match self.more(page) {
            Some(where_in) => format!("{}   {where_in}", self.title),
            None => self.title.clone(),
        }
    }

    /// Scroll down by `rows`, stopping at the last page rather than past it.
    pub fn down(&mut self, rows: usize, page: usize) {
        let last = self.rows.len().saturating_sub(page);
        self.top = (self.top + rows).min(last);
    }

    pub fn up(&mut self, rows: usize) {
        self.top = self.top.saturating_sub(rows);
    }

    pub fn bottom(&mut self, page: usize) {
        self.top = self.rows.len().saturating_sub(page);
    }

    /// The rows to draw, for a viewport `page` tall. The row under the pointer is drawn inverted,
    /// the way the fold handles and the usage badge invert, so a selectable row shows it is one.
    #[must_use]
    pub fn showing(&self, page: usize) -> Vec<Line<'static>> {
        if self.rows.is_empty() {
            return vec![Line::from(self.empty.clone())];
        }
        self.rows
            .iter()
            .enumerate()
            .skip(self.top)
            .take(page)
            .map(|(at, line)| {
                if Some(at) == self.hover {
                    lit(line)
                } else {
                    line.clone()
                }
            })
            .collect()
    }

    /// The panel, framed and scanning, `width` columns across. It wears the same travelling-light
    /// ring as the prompt box, which is how it says it holds the keyboard while the prompt goes dark.
    #[must_use]
    pub fn framed(
        &self,
        width: u16,
        page: usize,
        tick: usize,
        scan: crate::border::Scan,
    ) -> Vec<Line<'static>> {
        // The heading and the blank under it are content rows, so the light runs past them rather
        // than round a hole in the box.
        let mut body = vec![
            Line::from(Span::styled(
                self.heading(page),
                Style::default()
                    .fg(crate::colour::hint())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(String::new()),
        ];
        body.extend(self.showing(page));

        // Padded out to the full page rather than shrunk to fit, so the window does not jump size.
        let content = page + 2;
        body.resize(content, Line::from(String::new()));
        let (top, bottom) = crate::border::edges(width, content, tick, scan);
        let mut out = Vec::with_capacity(content + 2);
        out.push(top);
        for (row, line) in body.into_iter().enumerate() {
            let (left, right) = crate::border::side(width, content, row, tick, scan);
            let mut spans = vec![left, Span::raw(" ")];
            let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            spans.extend(line.spans);
            let room = usize::from(width).saturating_sub(3);
            if used < room {
                spans.push(Span::raw(" ".repeat(room - used)));
            }
            spans.push(right);
            out.push(Line::from(spans));
        }
        out.push(bottom);
        out
    }
    /// Whether there is anything above or below what is shown, for the title.
    #[must_use]
    pub fn more(&self, page: usize) -> Option<String> {
        if self.rows.len() <= page {
            return None;
        }
        let last = (self.top + page).min(self.rows.len());
        Some(format!("{}–{} of {}", self.top + 1, last, self.rows.len()))
    }
}

/// A copy of `line` with every span inverted: the whole row reads as one highlighted block.
fn lit(line: &Line<'static>) -> Line<'static> {
    let spans = line
        .spans
        .iter()
        .map(|span| {
            Span::styled(
                span.content.clone(),
                span.style.add_modifier(Modifier::REVERSED),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

#[cfg(test)]
#[path = "pane/placing.rs"]
mod placing;
