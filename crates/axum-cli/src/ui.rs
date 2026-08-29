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
use axum_tui::metric;
use axum_tui::{complete, prompt, status, transcript};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;

/// Rows the chrome below the transcript occupies at its smallest: status, prompt, footer.
#[must_use]
pub fn chrome_rows() -> u16 {
    metric::status_rows() + metric::prompt_min_rows() + metric::footer_rows()
}

/// Rows the live region should claim on a terminal `rows` tall.
///
/// A third of the screen at most: the transcript's home is scrollback, and a viewport that
/// fills the terminal defeats the point of rendering into it.
#[must_use]
pub fn initial_height(rows: u16) -> u16 {
    let live = metric::share(rows, metric::live_share()).min(metric::live_rows());
    (live + metric::status_rows() + metric::prompt_min_rows() + metric::footer_rows())
        .min(rows.saturating_sub(1))
}

/// Draw the live region.
pub fn draw(frame: &mut Frame<'_>, app: &mut App, footer_data: &FooterData, mode: Mode) {
    let area = frame.area();
    let mut hidden_below = 0usize;
    let rows = area.height;

    // The scan says what the session is doing, which is why it is chosen here rather than in the
    // prompt: this is the only place that knows about the turn as well as the text.
    let scan = if !app.connected {
        axum_tui::border::Scan::Off
    } else if app.is_busy() {
        axum_tui::border::Scan::Working
    } else if app.editor.is_blank() {
        axum_tui::border::Scan::Resting
    } else {
        axum_tui::border::Scan::Holding
    };
    let prompt_lines = prompt::render(&app.editor, area.width, rows, app.scan_tick(), scan);
    let prompt_rows = (prompt_lines.len() as u16)
        .min(rows.saturating_sub(metric::footer_rows() + metric::status_rows() + 1))
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
        .min(rows.saturating_sub(metric::footer_rows() + metric::status_rows() + prompt_rows + 1));

    // The "there is more below" rule sits directly on top of the prompt box, under the status
    // line rather than above it. Drawn at the bottom of the transcript instead, it had the
    // status row between it and the box — a blank row most of the time, which reads as the rule
    // marking some other edge than the one you are about to type at.
    // "Not following" is exactly "you have scrolled away from the newest output", which is when
    // this is worth saying — and it avoids asking how much is below, which depends on a height
    // this row is about to change.
    let more_rows = u16::from(matches!(mode, Mode::Alt) && !app.scrollback.is_following());
    let [
        live_area,
        status_area,
        rule_area,
        prompt_area,
        popup_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(metric::status_rows()),
        Constraint::Length(more_rows),
        Constraint::Length(prompt_rows),
        Constraint::Length(popup_rows),
        Constraint::Length(metric::footer_rows()),
    ])
    .areas(area);

    match mode {
        // Inline: the terminal owns the history, so only what has not settled is drawn, and
        // the tail of it — a message longer than the region streams past, and the newest text
        // is the part being written.
        Mode::Inline => {
            let live = transcript::render(app.live(), area.width, app.detail);
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
            let laid = transcript::laid_out(app.entries(), area.width, app.detail, &app.flipped);
            app.owners = laid.owners;
            app.scrollback.set_lines(laid.lines);
            // Only the top edge takes a row out of the transcript; the bottom one has a slot of
            // its own directly above the prompt.
            let above = app.scrollback.hidden_above() > 0;
            let live_area = Rect {
                y: live_area.y + u16::from(above),
                height: live_area.height.saturating_sub(u16::from(above)),
                ..live_area
            };
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
            // The rows the transcript actually landed on, which is what a click is measured
            // against. Bottom-anchored, so a short transcript does not start at the top.
            app.live_rows = anchored.y..anchored.y + anchored.height;
            frame.render_widget(Paragraph::new(view), anchored);
            if above {
                frame.render_widget(
                    Paragraph::new(status::more(area.width)),
                    Rect {
                        y: live_area.y - 1,
                        height: 1,
                        ..live_area
                    },
                );
            }
        }
    }

    // Composed rather than passed in: the scroll note is a fact about where the reader is
    // looking, which the status line has no business knowing how to compute.
    let mut status_line = status::working(app.status(), app.tick, app.connected, app.elapsed());
    if matches!(mode, Mode::Alt) {
        status_line.spans.extend(status::scrolled(hidden_below));
    }
    if !app.connected {
        status_line.spans.extend(status::queued(app.queued));
    }
    frame.render_widget(Paragraph::new(status_line), status_area);
    if more_rows > 0 {
        frame.render_widget(Paragraph::new(status::more(area.width)), rule_area);
    }
    frame.render_widget(Paragraph::new(prompt_lines), prompt_area);

    if let Some(picker) = &app.picker {
        frame.render_widget(
            Paragraph::new(axum_tui::picker::render(picker, area.width)),
            popup_area,
        );
    } else if let Some(popup) = &app.completion {
        frame.render_widget(
            Paragraph::new(complete::render(popup, area.width)),
            popup_area,
        );
    }
    frame.render_widget(
        Paragraph::new(footer::render(footer_data, area.width)),
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

    // Row 0 of the prompt area is the box's top edge, so the text begins one row down.
    let row = u16::try_from(cursor_row.saturating_sub(offset)).unwrap_or(0) + 1;
    if row >= area.height {
        return;
    }
    // And two columns in: the left bar, then the padding column. This used to be `area.x + col`,
    // which was right while the prompt was two rules and text starting in column zero — with
    // the box it put the terminal's cursor two cells to the left of the one drawn into the
    // line, so the caret and the block disagreed about where you were typing.
    let col = u16::try_from(cursor_col)
        .unwrap_or(u16::MAX)
        .saturating_add(metric::gutter());
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
        assert_eq!(initial_height(200), metric::live_rows() + 6);
    }

    #[test]
    fn a_short_terminal_never_claims_more_rows_than_it_has() {
        for rows in 3..12_u16 {
            assert!(initial_height(rows) < rows, "{rows} rows");
        }
    }
}

/// The edge says the transcript continues, so "is this the end" is answered by looking rather
/// than by reading a count somewhere else on the screen.
#[cfg(test)]
mod continues_past_the_edge {
    use super::*;
    use axum_proto::{Cursor, HarnessEvent, MessageId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// A 60x16 screen holding a conversation `turns` long, scrolled up by `lines`.
    fn drawn(turns: usize, lines: usize) -> Vec<String> {
        let mut app = App::new();
        for n in 0..turns {
            app.apply(HarnessEvent::UserMessage {
                cursor: Cursor(n as u64 + 1),
                id: MessageId::new(format!("u{n}")),
                text: format!("question number {n}"),
            });
        }
        let footer = FooterData::default();
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).expect("test terminal");
        // Drawn once so the scrollback learns how tall its view is, then scrolled and redrawn.
        terminal
            .draw(|frame| draw(frame, &mut app, &footer, Mode::Alt))
            .expect("draw");
        if lines > 0 {
            app.scrollback.scroll_up(lines);
        }
        terminal
            .draw(|frame| draw(frame, &mut app, &footer, Mode::Alt))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .chunks(60)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    fn rules(rows: &[String]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| row.starts_with("─ ─ ─"))
            .map(|(at, _)| at)
            .collect()
    }

    #[test]
    fn a_transcript_that_fits_draws_no_rule() {
        let rows = drawn(1, 0);
        assert!(rules(&rows).is_empty(), "{rows:#?}");
    }

    #[test]
    fn more_below_draws_a_rule_under_the_transcript() {
        let rows = drawn(30, 20);
        assert!(
            !rules(&rows).is_empty(),
            "nothing marked the edge: {rows:#?}"
        );
    }

    #[test]
    fn scrolled_into_the_middle_marks_both_edges() {
        let rows = drawn(30, 10);
        assert_eq!(rules(&rows).len(), 2, "it runs off both ends: {rows:#?}");
    }

    #[test]
    fn the_lower_rule_sits_directly_on_the_prompt_box() {
        // Drawn at the bottom of the transcript it had the status row beneath it — blank most
        // of the time, so the rule looked like it marked some edge other than the one you are
        // about to type at.
        let rows = drawn(30, 10);
        let box_top = rows
            .iter()
            .position(|row| row.contains(char::from_u32(0x256D).expect("box corner")))
            .expect("the prompt is on screen");
        let lower = *rules(&rows).last().expect("a rule below");
        assert_eq!(lower + 1, box_top, "nothing goes between them: {rows:#?}");
    }

    #[test]
    fn a_rule_never_lands_on_the_prompt() {
        let rows = drawn(30, 20);
        let box_top = rows
            .iter()
            .position(|row| row.contains('╭'))
            .expect("the prompt is on screen");
        for at in rules(&rows) {
            assert!(at < box_top, "a rule landed on the prompt: {rows:#?}");
        }
    }
}
