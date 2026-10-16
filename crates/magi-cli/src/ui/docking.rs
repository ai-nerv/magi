//! Running blocks on screen: the pinned ones above the prompt, the chip that counts them, the rows a
//! block with the keyboard is measured against, and the flash that says which one was picked.

use super::*;
use magi_proto::ToolCallId;
use ratatui::style::Modifier;
use ratatui::text::Line;

/// Each pinned block's rows above the prompt, at `width`.
pub(super) fn lines(app: &App, width: u16) -> Vec<(ToolCallId, Vec<Line<'static>>)> {
    app.docking()
        .into_iter()
        .map(|(live, open, id)| (id, transcript::docked(&live, open, width)))
        .collect()
}

/// How many rows the pinned blocks take.
pub(super) fn rows(docked: &[(ToolCallId, Vec<Line<'static>>)]) -> u16 {
    u16::try_from(docked.iter().map(|(_, lines)| lines.len()).sum::<usize>()).unwrap_or(u16::MAX)
}

/// Draw the pinned blocks into `area`, lighting `lit`, and note where each landed.
pub(super) fn draw(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    docked: Vec<(ToolCallId, Vec<Line<'static>>)>,
    lit: Option<&ToolCallId>,
) {
    app.docked.clear();
    let hover = app.dock_hover.clone();
    let mut y = area.y;
    for (id, mut lines) in docked {
        let height = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .min((area.y + area.height).saturating_sub(y));
        if let Some((_, column)) = hover.as_ref().filter(|(over, _)| *over == id)
            && let Some(header) = lines.first_mut()
        {
            transcript::hovered(header, *column);
        }
        if lit == Some(&id) {
            flash(&mut lines);
        }
        let at = Rect {
            x: area.x,
            y,
            width: area.width,
            height,
        };
        frame.render_widget(Paragraph::new(lines), at);
        app.docked.push((id, at));
        y += height;
    }
}

/// The badge with the running-blocks chip in front of it, when anything runs and there is a badge
/// to stand beside.
pub(super) fn badge(app: &App, cost: String) -> String {
    let running = app.running().count();
    if running == 0 || cost.is_empty() {
        return cost;
    }
    format!("{running}{}{cost}", fold::SEP)
}

/// Where the rows of `held` are on screen, while it has the keyboard: the float it is viewed in,
/// the dock it is pinned in, or its own block in the conversation.
pub(super) fn held_rect(
    app: &App,
    held: &crate::app::surfacing::Surfacing,
    area: Rect,
) -> Option<Rect> {
    if held.viewing {
        return Some(magi_tui::pane::Pane::surface_inside(
            magi_tui::pane::Pane::area(area),
        ));
    }
    if held.pinned {
        let (_, at) = app.docked.iter().find(|(id, _)| *id == held.id)?;
        // Under its header, and above the row of fill that closes it.
        return Some(Rect {
            x: transcript::body_left(),
            y: at.y + 1,
            width: transcript::body_width(area.width),
            height: at.height.saturating_sub(2),
        });
    }
    app.block_rect(held.call.as_ref()?, area.width)
}

/// Light the rows of `lit`'s block that are on screen, `shown` being the transcript rows `view` holds.
pub(super) fn flash_in_view(
    app: &App,
    lit: &ToolCallId,
    view: &mut [Line<'static>],
    shown: &std::ops::Range<usize>,
) {
    let Some((start, rows)) = app
        .surfaces
        .iter()
        .find(|held| held.id == *lit)
        .and_then(|held| held.call.as_ref())
        .and_then(|call| app.stack.find(call))
        .and_then(|nth| app.stack.block(nth))
    else {
        return;
    };
    for at in start..start + rows.len() {
        if let Some(row) = at
            .checked_sub(shown.start)
            .and_then(|into| view.get_mut(into))
        {
            flash(std::slice::from_mut(row));
        }
    }
}

/// Invert every cell of `lines`.
fn flash(lines: &mut [Line<'static>]) {
    for span in lines.iter_mut().flat_map(|line| line.spans.iter_mut()) {
        span.style = if span.style.add_modifier.contains(Modifier::REVERSED) {
            span.style.remove_modifier(Modifier::REVERSED)
        } else {
            span.style.add_modifier(Modifier::REVERSED)
        };
    }
}
