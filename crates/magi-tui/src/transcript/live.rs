//! A block a tool is drawing in: its live rows in the conversation, and pinned above the prompt.

use super::tool::STEP;
use super::{Detail, clip};
use crate::colour;
use ratatui::text::{Line, Span};

/// How wide the body of a tool's block is on a screen `width` columns wide: what a surface drawn in
/// its own block is given.
#[must_use]
pub fn body_width(width: u16) -> u16 {
    width.saturating_sub(u16::try_from(super::frame::MARGIN * 2 + STEP).unwrap_or(6))
}

/// The column the body of a tool's block starts in.
#[must_use]
pub fn body_left() -> u16 {
    u16::try_from(super::frame::MARGIN + STEP).unwrap_or(4)
}

/// A call whose tool draws in its own block: its header, and the tool's last frame as the body,
/// what it is until the first frame arrives, or where it is drawn while it is drawn elsewhere.
pub(super) fn block(live: &super::Live<'_>, width: u16, fold: Option<Detail>) -> super::Block {
    let mark = if live.focused {
        (crate::glyph::focused(), colour::accent())
    } else {
        (crate::glyph::running(), colour::tool_title())
    };
    let head = super::Head {
        label: live.tool.to_owned(),
        ink: Some(colour::tool_title()),
        mark: Some(mark),
        fold,
        pin: live.pin,
        copy: false,
    };
    let mut block = super::Block::new(head, colour::tool_bg());
    let style = block.style();
    let lead = super::frame::MARGIN + STEP;
    let room = usize::from(body_width(width));
    let said = |text: &str| {
        vec![Line::from(Span::styled(
            clip(text, room),
            style.fg(colour::dim()),
        ))]
    };
    let rows: Vec<Line<'static>> = match live.away {
        Some(away) => said(away),
        None if live.drawn.is_empty() => said(live.title),
        None => crate::painted::lines(live.drawn, style),
    };
    block.rows = rows
        .into_iter()
        .map(|line| {
            let kept = Line::from(super::shell::clipped(line.spans, room));
            super::frame::inside(kept, width, style, lead)
        })
        .collect();
    block
}

/// A pinned block above the prompt: open, the block with its rows running; folded, its header and
/// its title alone.
#[must_use]
pub fn docked(live: &super::Live<'_>, open: bool, width: u16) -> Vec<Line<'static>> {
    let fold = Some(if open { Detail::Full } else { Detail::Preview });
    if open {
        return block(live, width, fold).lines(width);
    }
    let folded = super::Live {
        drawn: &[],
        away: None,
        ..*live
    };
    let mut lines = block(&folded, width, fold).lines(width);
    lines.pop();
    lines
}
