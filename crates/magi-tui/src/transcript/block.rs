//! A block: one transcript entry drawn as a rectangle of its own colour. Its header is the first
//! row inside it, its rule in the screen's own colour, and a row of the fill closes it.
//!
//! ```text
//!  ▒══[ NAME ]══[ ✓ ]══════════════════════[ ⧉ ]══[ ▸ ]══▒
//!  ▒  …the block's own rows…                              ▒
//!  ▒                                                      ▒
//! ```

use super::{Detail, frame};
use crate::glyph;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// What a block's header carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    pub label: String,
    /// The label's own colour; without one it is drawn like the rule.
    pub ink: Option<Color>,
    /// A chip beside the label, in a colour of its own.
    pub mark: Option<(&'static str, Color)>,
    /// The fold handle, on a block that folds.
    pub fold: Option<Detail>,
    /// The pin chip, on a running block: whether it is pinned already.
    pub pin: Option<bool>,
    pub copy: bool,
}

/// One entry of the transcript, as it is drawn.
#[derive(Debug, Clone)]
pub struct Block {
    pub head: Head,
    pub fill: Color,
    /// The rows under the header, each already set inside the fill.
    pub rows: Vec<Line<'static>>,
}

impl Block {
    #[must_use]
    pub const fn new(head: Head, fill: Color) -> Self {
        Self {
            head,
            fill,
            rows: Vec::new(),
        }
    }

    /// The ground every row of the block is painted on.
    #[must_use]
    pub fn style(&self) -> Style {
        Style::default().bg(self.fill)
    }

    /// The block at `width`: its header, then its rows and a row of fill closing them.
    #[must_use]
    pub fn lines(self, width: u16) -> Vec<Line<'static>> {
        let style = self.style();
        let mut out = vec![header(&self.head, self.fill, width)];
        if !self.rows.is_empty() {
            out.extend(self.rows);
            out.push(frame::breath(width, style));
        }
        out
    }
}

/// Whether `line` is a block's header.
#[must_use]
pub fn is_header(line: &Line<'_>) -> bool {
    line.spans.iter().any(|span| {
        span.style.add_modifier.contains(Modifier::REVERSED)
            && span.content.starts_with(glyph::block_edge())
    })
}

/// The colour behind a cell drawn in `style`.
#[must_use]
pub fn ground(style: Style) -> Option<Color> {
    if style.add_modifier.contains(Modifier::REVERSED) {
        style.fg
    } else {
        style.bg
    }
}

/// The header row: one column of fill either side of the rule.
fn header(head: &Head, fill: Color, width: u16) -> Line<'static> {
    let (outer, room) = frame::fill_of(width);
    let fill_cell = Span::styled(" ", Style::default().bg(fill));
    let mut spans = vec![Span::raw(" ".repeat(outer)), fill_cell.clone()];
    spans.extend(rule(head, fill, room.saturating_sub(2)));
    spans.push(fill_cell);
    spans.push(Span::raw(" ".repeat(outer)));
    Line::from(spans)
}

/// The rule across a header, `room` columns wide. Chips that do not fit are left off: the copy
/// chip first, then the mark, then the label is cut, and the handle last.
fn rule(head: &Head, fill: Color, room: usize) -> Vec<Span<'static>> {
    let knock = Style::default().fg(fill).add_modifier(Modifier::REVERSED);
    let edge = |n: usize| Span::styled(glyph::block_edge().repeat(n), knock);
    let chip = |text: &str| crate::wrap::columns(text) + 4;
    let lead = 2.min(room);

    let handle = head
        .fold
        .map(|detail| match detail {
            Detail::Preview => glyph::expand(),
            Detail::Full => glyph::collapse(),
        })
        .filter(|handle| lead + chip(handle) + 2 <= room);
    let held = handle.map_or(0, |handle| chip(handle) + 2);
    let pin = head
        .pin
        .map(|pinned| if pinned { glyph::unpin() } else { glyph::pin() })
        .filter(|pin| lead + chip(pin) + 2 + held <= room);
    let held = held + pin.map_or(0, |pin| chip(pin) + 2);
    let label_room = room.saturating_sub(lead + held + 4);
    let label =
        (!head.label.is_empty() && label_room > 0).then(|| super::clip(&head.label, label_room));
    let named = label.as_deref().map_or(0, chip);
    let mark = head
        .mark
        .filter(|(mark, _)| lead + named + 2 + chip(mark) + held <= room);
    let marked = mark.map_or(0, |(mark, _)| 2 + chip(mark));
    let copied = chip(glyph::copy()) + 2;
    let copy = head.copy && lead + named + marked + copied + held <= room;
    let copied = if copy { copied } else { 0 };
    let run = room.saturating_sub(lead + named + marked + copied + held);

    let mut spans = vec![edge(lead)];
    if let Some(label) = label {
        spans.push(Span::styled("[ ", knock));
        let ink = head
            .ink
            .map_or(knock, |ink| Style::default().fg(ink).bg(fill));
        spans.push(Span::styled(label, ink.add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(" ]", knock));
    }
    if let Some((mark, ink)) = mark {
        spans.push(edge(2));
        spans.push(Span::styled("[ ", knock));
        spans.push(Span::styled(mark, Style::default().fg(ink).bg(fill)));
        spans.push(Span::styled(" ]", knock));
    }
    spans.push(edge(run));
    for chip in [copy.then(glyph::copy), pin, handle].into_iter().flatten() {
        spans.push(Span::styled(format!("[ {chip} ]"), knock));
        spans.push(edge(2));
    }
    spans
}

#[cfg(test)]
#[path = "block/tests.rs"]
mod tests;
