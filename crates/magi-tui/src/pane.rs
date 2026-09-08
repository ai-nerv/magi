//! The info pane: what magi knows, shown in the middle, on request.
//!
//! **One primitive, many views.** `:trace` and `:graph` are the first two; cost breakdowns,
//! session info and whatever else magi can answer about itself go in the same panel with a
//! different title and a different list of rows. Adding one is a function that returns
//! `Vec<Line>` and a line in the command table — not a new renderer, not a new key handler, not
//! a new place on the screen for a reader to learn.
//!
//! # Not the surface, and deliberately so
//!
//! magi already has a region that opens over the prompt and takes the keyboard: the *menu* slot,
//! shared by pickers, permission questions, completions and the rows a **tool** holds through
//! `casper surface`. That slot is for things *asking for the keyboard* — something wants an
//! answer before the session goes on — and it lives in the prompt box because that is where a
//! reader already looks for a question.
//!
//! This is the other thing entirely:
//!
//! | | the menu slot | the info pane |
//! |---|---|---|
//! | who opens it | a tool, or a question the session must ask | the person, by name |
//! | what it wants | an answer | nothing |
//! | where it draws | inside the prompt box | the middle of the screen |
//! | who may drive it | external tools, over the surface protocol | magi alone |
//! | blocks the turn | yes, while it holds the keyboard | no |
//!
//! Keeping them apart is the point. A tool that could open a panel in the middle of the screen
//! would be a tool that can cover the conversation whenever it likes, and the surface protocol
//! is deliberately the narrower grant — rows inside the box, for as long as the call lasts. The
//! info pane is magi showing you magi, and nothing outside this process reaches it.
//!
//! **No pty, and no alternate screen.** Both were considered. A pty is for running somebody
//! else's program and reading what it drew; there is no program here — the rows are magi's own
//! data, in magi's own frame, and a pty would add a process, an escape-sequence parser and a
//! second thing that can hang, to draw a list magi already holds. An alternate screen would take
//! the conversation away, and a view opened for a moment should give it back untouched.
//!
//! # What it does
//!
//! It draws over the transcript rather than pushing it aside, because the transcript is still
//! what the person is reading and a layout that reflowed the conversation to make room would
//! move the thing they were looking at.
//!
//! **Rows, not widgets.** What goes in a pane is a list of lines somebody else built, the same
//! way the transcript and the overlays work here. That keeps the drawing in one place.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The share of the screen a float takes, in percent.
///
/// Wide enough for a path and a description side by side, and short enough that the conversation
/// is still visible around it — a float that covered everything would be a screen, and a screen
/// is a thing you navigate rather than glance at.
const WIDTH: u16 = 80;
/// The share of the screen's height.
const HEIGHT: u16 = 70;

/// A view drawn in the middle of the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    /// What it is, drawn in the border.
    pub title: String,
    /// What it says, already laid out.
    pub rows: Vec<Line<'static>>,
    /// The first row shown, for scrolling.
    pub top: usize,
    /// Whether to show the newest end rather than the oldest.
    ///
    /// A flag rather than a scroll position set at construction, because the position depends on
    /// how tall the panel is and only the renderer knows that. A trace opened at the first thing
    /// that ever happened would need scrolling before it answered anything.
    pub follow: bool,
    /// What to say when there is nothing to show.
    ///
    /// Its own field rather than a row, because an empty view usually wants to say *how to make
    /// it non-empty* and that is not the same voice as its contents.
    pub empty: String,
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
        }
    }

    /// What to say when it holds nothing.
    #[must_use]
    pub fn saying(mut self, empty: impl Into<String>) -> Self {
        self.empty = empty.into();
        self
    }

    /// Open at the newest end.
    #[must_use]
    pub fn following(mut self) -> Self {
        self.follow = true;
        self
    }

    /// Settle the scroll position for a viewport `page` tall.
    ///
    /// Called by the renderer, which is the only thing that knows how tall the panel is. Once
    /// settled the flag is cleared, so scrolling up afterwards is not undone on the next draw.
    pub fn settle(&mut self, page: usize) {
        if self.follow {
            self.bottom(page);
            self.follow = false;
        }
    }

    /// Where it sits, given the whole screen.
    ///
    /// Centred by construction: the same margin on both sides, so a screen with an odd number of
    /// spare columns leans left by one rather than drifting as the terminal is resized.
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

    /// How many rows of content fit, given the whole screen.
    ///
    /// Four go elsewhere: two to the border, and two to the heading and the blank line under it.
    /// The heading is inside rather than in the border because a border title is drawn *in* the
    /// line, which forces the frame to break for it — and a rule that breaks for a word reads as
    /// a damaged box rather than a labelled one.
    #[must_use]
    pub fn page(screen: Rect) -> usize {
        Self::area(screen).height.saturating_sub(4) as usize
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
    ///
    /// Scrolling past the end is how a list ends up showing an empty box with a scrollbar: the
    /// content is above the viewport and there is nothing to say so.
    pub fn down(&mut self, rows: usize, page: usize) {
        let last = self.rows.len().saturating_sub(page);
        self.top = (self.top + rows).min(last);
    }

    /// Scroll up by `rows`.
    pub fn up(&mut self, rows: usize) {
        self.top = self.top.saturating_sub(rows);
    }

    /// Go to the newest end.
    pub fn bottom(&mut self, page: usize) {
        self.top = self.rows.len().saturating_sub(page);
    }

    /// The rows to draw, for a viewport `page` tall.
    #[must_use]
    pub fn showing(&self, page: usize) -> Vec<Line<'static>> {
        if self.rows.is_empty() {
            return vec![Line::from(self.empty.clone())];
        }
        self.rows
            .iter()
            .skip(self.top)
            .take(page)
            .cloned()
            .collect()
    }

    /// The panel, framed and scanning, `width` columns across.
    ///
    /// **The same ring the prompt box wears.** [`crate::border`] addresses a border as one ring of
    /// cells with a light travelling it, and nothing about that is specific to the prompt — so the
    /// pane wears it too, and the two look like parts of one program rather than one box drawn by
    /// hand next to another.
    ///
    /// **And it is how the pane says it has the focus.** While a pane is open it owns the
    /// keyboard: the arrows scroll it, escape closes it, and anything else is swallowed rather
    /// than reaching a prompt nobody can see. The scan moves here and the prompt goes dark, so
    /// which box is listening is something you can see rather than something you find out by
    /// typing into the one that is not.
    #[must_use]
    pub fn framed(
        &self,
        width: u16,
        page: usize,
        tick: usize,
        scan: crate::border::Scan,
    ) -> Vec<Line<'static>> {
        // The heading and the blank under it are content rows like any other, so the sides stay on
        // the ring and the light runs past them rather than round a hole in the box.
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

        let content = body.len();
        let (top, bottom) = crate::border::edges(width, content, tick, scan);
        let mut out = Vec::with_capacity(content + 2);
        out.push(top);
        for (row, line) in body.into_iter().enumerate() {
            let (left, right) = crate::border::side(width, content, row, tick, scan);
            let mut spans = vec![left, Span::raw(" ")];
            let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            spans.extend(line.spans);
            // Padded to the column the right-hand side sits in, so a short row does not leave the
            // border ragged and a long one does not push it off the end.
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
    /// Whether there is anything above or below what is shown.
    ///
    /// For the title, which is the only place there is room to say it — a float that silently
    /// holds four hundred rows and shows twenty looks like a float that holds twenty.
    #[must_use]
    pub fn more(&self, page: usize) -> Option<String> {
        if self.rows.len() <= page {
            return None;
        }
        let last = (self.top + page).min(self.rows.len());
        Some(format!("{}–{} of {}", self.top + 1, last, self.rows.len()))
    }
}

#[cfg(test)]
#[path = "pane/placing.rs"]
mod placing;
