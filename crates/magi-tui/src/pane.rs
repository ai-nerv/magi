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

/// The heading's own row inside the panel, and the column the first tab starts at: the border and
/// the space after it.
const HEADING: u16 = 1;
const INSET: u16 = 2;

/// Between one tab and the next.
const TAB_GAP: &str = " ";

/// Left of every row in a list: a bar beside the entry the cursor is on, blank beside the rest.
const GUTTER: &str = "┃ ";
const NO_GUTTER: &str = "  ";

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
    /// Consecutive rows naming the same target are one entry.
    pub picks: Vec<Option<String>>,
    /// The row the cursor is on, as an index into `rows`. The keys move it entry by entry, the
    /// pointer puts it where it points, and every row of its entry is lit.
    pub hover: Option<usize>,
    /// Bring the cursor's entry into view at the next draw, the one place the page is known.
    reveal: bool,
    /// The tabs across the heading row, empty for a float that is one view. Tab and shift-tab move
    /// between them; the view that owns the float rebuilds its rows for whichever is current.
    pub tabs: Vec<String>,
    pub tab: usize,
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
            reveal: false,
            tabs: Vec::new(),
            tab: 0,
        }
    }

    /// Put `tabs` across the heading, with `at` current. An `at` past the end lands on the last.
    #[must_use]
    pub fn tabbed(mut self, tabs: Vec<String>, at: usize) -> Self {
        self.tab = at.min(tabs.len().saturating_sub(1));
        self.tabs = tabs;
        self
    }

    /// Move to the next tab, or the one before, wrapping. False when there is nowhere to go, so a
    /// float with one view lets the key mean whatever it meant before.
    pub fn step_tab(&mut self, forward: bool) -> bool {
        if self.tabs.len() < 2 {
            return false;
        }
        let last = self.tabs.len() - 1;
        self.tab = if forward {
            if self.tab == last { 0 } else { self.tab + 1 }
        } else if self.tab == 0 {
            last
        } else {
            self.tab - 1
        };
        true
    }

    /// Where each tab sits along the heading row, as columns from the panel's left edge, for a
    /// pointer that wants to press one. Parallel to `tabs`.
    #[must_use]
    pub fn tab_columns(&self) -> Vec<std::ops::Range<u16>> {
        let mut at = INSET;
        self.tabs
            .iter()
            .enumerate()
            .map(|(nth, name)| {
                if nth > 0 {
                    at += u16::try_from(TAB_GAP.chars().count()).unwrap_or(0);
                }
                let width = u16::try_from(name.chars().count() + 2).unwrap_or(0);
                let range = at..at + width;
                at += width;
                range
            })
            .collect()
    }

    /// Which tab a press `column` cells from the panel's left edge, on row `HEADING`, is on.
    #[must_use]
    pub fn tab_at(&self, row_in_panel: u16, column_in_panel: u16) -> Option<usize> {
        if row_in_panel != HEADING {
            return None;
        }
        self.tab_columns()
            .into_iter()
            .position(|at| at.contains(&column_in_panel))
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

    /// What the cursor is on.
    #[must_use]
    pub fn chosen(&self) -> Option<&str> {
        self.picks.get(self.hover?)?.as_deref()
    }

    /// Put the cursor on the row the pointer is over, `row_in_panel` cells below the top edge, when
    /// that row selects something; anywhere else it stays where it was. Says whether the lit entry
    /// changed, so a move within one entry skips a redraw.
    pub fn hover_at(&mut self, row_in_panel: u16) -> bool {
        let Some(at) = row_in_panel
            .checked_sub(HEAD)
            .map(|content| self.top + usize::from(content))
            .filter(|at| self.picks.get(*at).is_some_and(Option::is_some))
        else {
            return false;
        };
        let was = self.lit_rows();
        self.hover = Some(at);
        was != self.lit_rows()
    }

    /// Put the cursor on `id`'s entry. False when nothing here selects it.
    pub fn point_at(&mut self, id: &str) -> bool {
        let Some(at) = self
            .picks
            .iter()
            .position(|pick| pick.as_deref() == Some(id))
        else {
            return false;
        };
        self.hover = Some(at);
        self.reveal = true;
        true
    }

    /// Move the cursor one entry on, or back; onto the first entry when it is on none. Says whether
    /// it moved, so a key that finds the end of the entries can scroll the rows past them instead.
    pub fn step(&mut self, forward: bool) -> bool {
        let starts = self.starts();
        if starts.is_empty() {
            return false;
        }
        let was = self.hover;
        let now = self
            .hover
            .and_then(|at| starts.iter().rposition(|start| *start <= at));
        let next = match now {
            None => 0,
            Some(nth) if forward => (nth + 1).min(starts.len() - 1),
            Some(nth) => nth.saturating_sub(1),
        };
        self.hover = Some(starts[next]);
        self.reveal = true;
        was != self.hover
    }

    /// The cursor onto the first entry, or the last.
    pub fn first(&mut self) {
        self.hover = self.starts().first().copied().or(self.hover);
        self.reveal = true;
    }

    pub fn last(&mut self) {
        self.hover = self.starts().last().copied().or(self.hover);
        self.reveal = true;
    }

    /// The first row of every entry: wherever the target changes to a new one.
    fn starts(&self) -> Vec<usize> {
        self.picks
            .iter()
            .enumerate()
            .filter(|(at, pick)| pick.is_some() && (*at == 0 || self.picks[at - 1] != **pick))
            .map(|(at, _)| at)
            .collect()
    }

    /// The rows of the entry the cursor is on; empty when it is on none.
    fn lit_rows(&self) -> std::ops::Range<usize> {
        let Some(at) = self
            .hover
            .filter(|at| self.picks.get(*at).is_some_and(Option::is_some))
        else {
            return 0..0;
        };
        let pick = &self.picks[at];
        let from = (0..at)
            .rev()
            .take_while(|row| self.picks[*row] == *pick)
            .last()
            .unwrap_or(at);
        let to = (at..self.picks.len())
            .take_while(|row| self.picks[*row] == *pick)
            .count();
        from..at + to
    }

    #[must_use]
    pub fn following(mut self) -> Self {
        self.follow = true;
        self
    }

    /// Settle the scroll position for a viewport `page` tall. Clears [`Pane::follow`], so a later
    /// scroll up is not undone on the next draw, and brings a cursor the keys moved into view.
    pub fn settle(&mut self, page: usize) {
        if self.follow {
            self.bottom(page);
            self.follow = false;
        }
        if self.reveal {
            let lit = self.lit_rows();
            if self.starts().first() == Some(&lit.start) {
                // The first entry shows what sits above it too: the heading of the list.
                self.top = 0;
            } else if lit.start < self.top {
                self.top = lit.start;
            } else if lit.end > self.top + page {
                self.top = lit.end.saturating_sub(page);
            }
            self.reveal = false;
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

    /// Where a surface that asked for the float draws: inside the border, a column in from each side.
    #[must_use]
    pub fn surface_inside(area: Rect) -> Rect {
        Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        }
    }

    /// The heading drawn inside the panel: the tabs where there are any, else what this is, and in
    /// both cases where in the rows you are.
    #[must_use]
    pub fn heading(&self, page: usize) -> Vec<Span<'static>> {
        let bold = Style::default()
            .fg(crate::colour::hint())
            .add_modifier(Modifier::BOLD);
        let mut spans = Vec::new();
        if self.tabs.is_empty() {
            spans.push(Span::styled(self.title.clone(), bold));
        } else {
            for (nth, name) in self.tabs.iter().enumerate() {
                if nth > 0 {
                    spans.push(Span::raw(TAB_GAP));
                }
                let style = if nth == self.tab {
                    bold.add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().fg(crate::colour::dim())
                };
                spans.push(Span::styled(format!(" {name} "), style));
            }
        }
        if let Some(where_in) = self.more(page) {
            spans.push(Span::styled(
                format!("   {where_in}"),
                Style::default().fg(crate::colour::hint()),
            ));
        }
        spans
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

    /// The rows to draw, for a viewport `page` tall. A list wears a gutter, and the entry the
    /// cursor is on is a band with a bar beside it.
    #[must_use]
    pub fn showing(&self, page: usize) -> Vec<Line<'static>> {
        if self.rows.is_empty() {
            return vec![Line::from(self.empty.clone())];
        }
        let lit = self.lit_rows();
        let listed = !self.picks.is_empty();
        self.rows
            .iter()
            .enumerate()
            .skip(self.top)
            .take(page)
            .map(|(at, line)| {
                if listed {
                    guttered(line, lit.contains(&at))
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
        let mut body = vec![Line::from(self.heading(page)), Line::from(String::new())];
        body.extend(self.showing(page));

        // Padded out to the full page rather than shrunk to fit, so the window does not jump size.
        let content = page + 2;
        body.resize(content, Line::from(String::new()));
        let (top, bottom) = crate::border::edges(width, content, tick, scan);
        let mut out = Vec::with_capacity(content + 2);
        out.push(top);
        let room = usize::from(width).saturating_sub(3);
        for (row, line) in body.into_iter().enumerate() {
            let (left, right) = crate::border::side(width, content, row, tick, scan);
            let fill = line.style;
            let (kept, used) = clipped(line.spans, room);
            let mut spans = vec![left, Span::raw(" ")];
            spans.extend(kept);
            if used < room {
                spans.push(Span::styled(" ".repeat(room - used), fill));
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

/// `line` behind a list's gutter. Lit, the bar and a band behind the whole row, carried to the
/// frame by the line's own style; otherwise two blank cells, so every row keeps one column.
fn guttered(line: &Line<'static>, lit: bool) -> Line<'static> {
    if !lit {
        let mut spans = vec![Span::raw(NO_GUTTER)];
        spans.extend(line.spans.iter().cloned());
        return Line::from(spans);
    }
    let band = Style::default().bg(crate::colour::pane_selected_bg());
    let mut spans = vec![Span::styled(GUTTER, band.fg(crate::colour::accent()))];
    spans.extend(
        line.spans
            .iter()
            .map(|span| Span::styled(span.content.clone(), span.style.patch(band))),
    );
    Line::from(spans).style(band)
}

/// `spans` cut to `room` columns, and how many of them they fill: a row too long for the panel
/// would push its right edge off the box.
fn clipped(spans: Vec<Span<'static>>, room: usize) -> (Vec<Span<'static>>, usize) {
    let mut used = 0;
    let mut out = Vec::new();
    for span in spans {
        let wide = span.content.chars().count();
        if used + wide <= room {
            used += wide;
            out.push(span);
            continue;
        }
        let kept: String = span.content.chars().take(room - used).collect();
        used = room;
        out.push(Span::styled(kept, span.style));
        break;
    }
    (out, used)
}

#[cfg(test)]
#[path = "pane/placing.rs"]
mod placing;

#[cfg(test)]
#[path = "pane/listing.rs"]
mod listing;
