//! Drawing the live region.
//!
//! Settled transcript blocks go to native scrollback via `insert_before`; everything that can
//! still change lives in an inline viewport that is redrawn each frame. That split is why the
//! transcript stays scrollable with the terminal's own scrollbar, as Pi's does.
//!
//! The stack below the transcript is Pi's, in Pi's order: status, then the bordered prompt
//! with its autocomplete beneath it, then the footer.
//!
//! The viewport is sized once, at startup, and never resized. Changing an inline viewport's
//! height means rebuilding the terminal, which asks the emulator where the cursor is; that
//! reply lands on stdin, where the key reader consumes it, and the resize times out. So the
//! region is a fixed budget and content is clipped into it instead.

use crate::app::App;
use crate::terminal::Mode;
use axum_tui::footer::{self, FooterData};
use axum_tui::{Theme, complete, prompt, status, transcript};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;

/// Rows the chrome below the transcript occupies at its smallest: status, prompt, footer.
pub const CHROME_ROWS: u16 = STATUS_ROWS + PROMPT_MIN_ROWS + FOOTER_ROWS;

/// Rows the footer always occupies.
///
/// One. It was two -- the directory on its own row above the stats -- and two rows of dim text
/// under the prompt is a lot of screen for something you glance at.
const FOOTER_ROWS: u16 = 1;

/// Rows the status line always occupies.
///
/// Two, even when idle. Pi's `IdleStatus` renders two blank lines and its `Loader` renders a
/// blank line above the spinner, so the layout does not jump the moment work starts.
const STATUS_ROWS: u16 = 2;

/// Rows the prompt claims when it holds a single line: rule, text, rule.
const PROMPT_MIN_ROWS: u16 = 3;

/// Live transcript rows to aim for, before the terminal's own height is taken into account.
const LIVE_TARGET: u16 = 10;

/// Rows the live region should claim on a terminal `rows` tall.
///
/// A third of the screen at most: the transcript's home is scrollback, and a viewport that
/// fills the terminal defeats the point of rendering into it.
#[must_use]
pub fn initial_height(rows: u16) -> u16 {
    let live = (rows / 3).clamp(4, LIVE_TARGET);
    (live + STATUS_ROWS + PROMPT_MIN_ROWS + FOOTER_ROWS).min(rows.saturating_sub(1))
}

/// Draw the live region.
pub fn draw(
    frame: &mut Frame<'_>,
    app: &mut App,
    footer_data: &FooterData,
    theme: &Theme,
    mode: Mode,
) {
    let area = frame.area();
    let mut hidden_below = 0usize;
    let rows = area.height;

    let prompt_lines = prompt::render(&app.editor, area.width, rows, theme);
    let prompt_rows = (prompt_lines.len() as u16)
        .min(rows.saturating_sub(FOOTER_ROWS + STATUS_ROWS + 1))
        .max(1);
    // One overlay slot. The two never open together — a list is opened by a command, and
    // running a command closes the popup that offered it.
    let popup_rows = app
        .picker
        .as_ref()
        .map_or_else(
            || {
                app.completion
                    .as_ref()
                    .map_or(0, complete::Completion::height)
            },
            axum_tui::picker::Picker::height,
        )
        .min(rows.saturating_sub(FOOTER_ROWS + STATUS_ROWS + prompt_rows + 1));

    let [live_area, status_area, prompt_area, popup_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(STATUS_ROWS),
        Constraint::Length(prompt_rows),
        Constraint::Length(popup_rows),
        Constraint::Length(FOOTER_ROWS),
    ])
    .areas(area);

    match mode {
        // Inline: the terminal owns the history, so only what has not settled is drawn, and
        // the tail of it — a message longer than the region streams past, and the newest text
        // is the part being written.
        Mode::Inline => {
            let live = transcript::render(app.live(), area.width, theme, app.detail);
            let shown = live
                .len()
                .saturating_sub(usize::from(live_area.height))
                .min(live.len());
            frame.render_widget(Paragraph::new(live[shown..].to_vec()), live_area);
        }
        // Alt: there is no terminal history to defer to, so the whole transcript is ours and
        // the reader's scroll position decides what shows.
        //
        // An empty session draws nothing. There was a greeting here — a name, the model, the
        // directory and four key hints — and it was right for somebody meeting the thing for
        // the first time and wrong for everybody after that. The prompt's own placeholder still
        // names `/`, which is the one line of it worth keeping.
        Mode::Alt => {
            let all = transcript::render(app.entries(), area.width, theme, app.detail);
            app.scrollback.set_lines(all);
            let view = app.scrollback.view(live_area.height).to_vec();
            // Bottom-aligned: a transcript grows towards the prompt, so a short one sits above
            // it rather than stranded at the top of the screen under a field of blank rows.
            let used = u16::try_from(view.len()).unwrap_or(live_area.height);
            let anchored = Rect {
                y: live_area.y + live_area.height.saturating_sub(used),
                height: used.min(live_area.height),
                ..live_area
            };
            hidden_below = app.scrollback.hidden_below(live_area.height);
            frame.render_widget(Paragraph::new(view), anchored);
        }
    }

    // Composed rather than passed in: the scroll note is a fact about where the reader is
    // looking, which the status line has no business knowing how to compute.
    let mut status_line =
        status::working(app.status(), app.tick, theme, app.connected, app.elapsed());
    if matches!(mode, Mode::Alt) {
        status_line
            .spans
            .extend(status::scrolled(hidden_below, theme));
    }
    if !app.connected {
        status_line.spans.extend(status::queued(app.queued, theme));
    }
    frame.render_widget(Paragraph::new(status_line), status_area);
    frame.render_widget(Paragraph::new(prompt_lines), prompt_area);

    if let Some(picker) = &app.picker {
        frame.render_widget(
            Paragraph::new(axum_tui::picker::render(picker, area.width, theme)),
            popup_area,
        );
    } else if let Some(popup) = &app.completion {
        frame.render_widget(
            Paragraph::new(complete::render(popup, area.width, theme)),
            popup_area,
        );
    }
    frame.render_widget(
        Paragraph::new(footer::render(footer_data, area.width, theme)),
        footer_area,
    );

    place_hardware_cursor(frame, app, prompt_area, rows);
}

/// Park the terminal cursor on the same cell the inverted block is drawn on.
///
/// The visible cursor is the inverted cell; this is for the terminal's own benefit â an IME
/// candidate window and a screen reader both follow the hardware cursor, not the colours.
fn place_hardware_cursor(frame: &mut Frame<'_>, app: &App, area: Rect, rows: u16) {
    if area.height < 2 {
        return;
    }
    let (cursor_row, cursor_col) = app.editor.cursor();
    let visible = prompt::visible_rows(rows);
    let offset = cursor_row.saturating_sub(visible.saturating_sub(1));

    // Row 0 of the prompt area is the rule, so the text begins one row down.
    let row = u16::try_from(cursor_row.saturating_sub(offset)).unwrap_or(0) + 1;
    if row >= area.height {
        return;
    }
    let col = u16::try_from(cursor_col).unwrap_or(u16::MAX);
    frame.set_cursor_position((area.x + col.min(area.width.saturating_sub(1)), area.y + row));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_live_region_gets_a_third_of_the_screen() {
        // 24 rows: 8 live + 2 status + 3 prompt + 1 footer.
        assert_eq!(initial_height(24), 14);
    }

    #[test]
    fn a_tall_terminal_does_not_get_a_tall_viewport() {
        assert_eq!(initial_height(200), LIVE_TARGET + 6);
    }

    #[test]
    fn a_short_terminal_never_claims_more_rows_than_it_has() {
        for rows in 3..12_u16 {
            assert!(initial_height(rows) < rows, "{rows} rows");
        }
    }
}
