//! Drawing the screen.
//!
//! axon owns every cell: the transcript is a buffer it keeps and scrolls, not lines handed to
//! the terminal. That is what the wheel, the scroll keys, the edge rule and clicking a block
//! open are all written against.
//!
//! The stack below the transcript is Pi's, in Pi's order: status, then the bordered prompt with
//! its autocomplete beneath it, then the footer.

use crate::app::App;

use axon_tui::footer::{self, FooterData};
use axon_tui::metric;
use axon_tui::{prompt, status, transcript};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;

/// Rows the chrome below the transcript occupies at its smallest: status, prompt, footer.
#[must_use]
pub fn chrome_rows() -> u16 {
    metric::status_rows() + metric::prompt_min_rows() + metric::footer_rows()
}

/// Draw the live region.
pub fn draw(frame: &mut Frame<'_>, app: &mut App, footer_data: &FooterData) {
    let area = frame.area();

    let rows = area.height;

    // The scan says what the session is doing, which is why it is chosen here rather than in the
    // prompt: this is the only place that knows about the turn as well as the text.
    let scan = if !app.connected {
        axon_tui::border::Scan::Off
    } else if app.is_busy() {
        axon_tui::border::Scan::Working
    } else if app.editor.is_blank() {
        axon_tui::border::Scan::Resting
    } else {
        axon_tui::border::Scan::Holding
    };

    // The "there is more below" rule sits directly on top of the prompt box, under the status
    // line rather than above it. Drawn at the bottom of the transcript instead, it had the
    // status row between it and the box — a blank row most of the time, which reads as the rule
    // marking some other edge than the one you are about to type at.
    // "Not following" is exactly "you have scrolled away from the newest output", which is when
    // this is worth saying — and it avoids asking how much is below, which depends on a height
    // this row is about to change.
    let more_rows = u16::from(!app.scrollback.is_following());

    // The menu goes inside the box, so the box is as tall as the two of them together and there
    // is no second region under it. What it may not do is take the whole screen: one row of
    // transcript stays, or a list opened mid-turn hides the turn it is about.
    let around = metric::footer_rows() + metric::status_rows() + more_rows + 1;
    let text_rows = prompt::text_rows(&app.editor, rows);
    let room = usize::from(rows.saturating_sub(around)).saturating_sub(text_rows + 3);
    let mut menu = app
        .overlay
        .as_ref()
        .map(|open| open.render(area.width.saturating_sub(metric::gutter() + 1)))
        .unwrap_or_default();
    menu.truncate(room);
    let prompt_lines = prompt::render(&app.editor, area.width, rows, app.scan_tick(), scan, &menu);
    let prompt_rows = u16::try_from(prompt_lines.len())
        .unwrap_or(u16::MAX)
        .min(rows.saturating_sub(around - 1))
        .max(1);

    let [live_area, status_area, rule_area, prompt_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(metric::status_rows()),
        Constraint::Length(more_rows),
        Constraint::Length(prompt_rows),
        Constraint::Length(metric::footer_rows()),
    ])
    .areas(area);

    // The whole transcript is ours and the reader's scroll position decides what shows.
    //
    // An empty session draws nothing. There was a greeting here — a name, the model, the
    // directory and four key hints — and it was right for somebody meeting the thing for the
    // first time and wrong for everybody after that. The prompt's own placeholder still names
    // `/`, which is the one line of it worth keeping.
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
    let hidden_below = app.scrollback.hidden_below(live_area.height);
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

    // Composed rather than passed in: the scroll note is a fact about where the reader is
    // looking, which the status line has no business knowing how to compute.
    let mut status_line = status::working(app.status(), app.tick, app.connected, app.elapsed());
    status_line.spans.extend(status::scrolled(hidden_below));
    if !app.connected {
        status_line.spans.extend(status::queued(app.queued));
    }
    frame.render_widget(Paragraph::new(status_line), status_area);
    if more_rows > 0 {
        frame.render_widget(Paragraph::new(status::more(area.width)), rule_area);
    }
    frame.render_widget(Paragraph::new(prompt_lines), prompt_area);
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

/// The edge says the transcript continues, so "is this the end" is answered by looking rather
/// than by reading a count somewhere else on the screen.
#[cfg(test)]
mod continues_past_the_edge {
    use super::*;
    use axon_proto::{Cursor, HarnessEvent, MessageId};
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
            .draw(|frame| draw(frame, &mut app, &footer))
            .expect("draw");
        if lines > 0 {
            app.scrollback.scroll_up(lines);
        }
        terminal
            .draw(|frame| draw(frame, &mut app, &footer))
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

/// What opens under the prompt opens *inside* it.
#[cfg(test)]
mod inside_the_box {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// A screen with `/mo` typed and the menu that opens on it.
    fn drawn() -> Vec<String> {
        let mut app = App::new();
        app.editor.insert_str("/mo");
        app.refresh_completion(&|_| Vec::new());
        assert!(app.overlay.is_some(), "the premise: `/mo` opens a menu");
        let footer = FooterData::default();
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, &mut app, &footer))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .chunks(60)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    #[test]
    fn every_row_of_the_menu_is_between_the_sides() {
        let rows = drawn();
        let menu: Vec<&String> = rows.iter().filter(|row| row.contains("/model")).collect();
        assert!(!menu.is_empty(), "nothing was offered: {rows:#?}");
        for row in menu {
            assert!(row.starts_with('│'), "{row:?} is outside the box");
            assert!(row.trim_end().ends_with('│'), "{row:?} is outside the box");
        }
    }

    #[test]
    fn a_rule_separates_the_text_from_what_it_opened() {
        let rows = drawn();
        let divider = rows
            .iter()
            .position(|row| row.starts_with('├'))
            .expect("a divider");
        let typed = rows
            .iter()
            .position(|row| row.contains("/mo "))
            .expect("what was typed");
        let offered = rows
            .iter()
            .position(|row| row.contains("/model"))
            .expect("what it offered");
        assert!(typed < divider && divider < offered, "{rows:#?}");
    }

    #[test]
    fn the_box_closes_under_the_menu_rather_than_above_it() {
        let rows = drawn();
        let bottom = rows
            .iter()
            .position(|row| row.starts_with('╰'))
            .expect("the box closes");
        let offered = rows
            .iter()
            .position(|row| row.contains("/model"))
            .expect("what it offered");
        assert!(offered < bottom, "the menu fell out of the box: {rows:#?}");
    }
}
