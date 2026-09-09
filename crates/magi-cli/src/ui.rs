//! Drawing the screen. magi owns every cell: the transcript is a buffer it keeps and scrolls, which
//! is what the wheel, the scroll keys, the edge rule and clicking a block open are written against.
//! Below it: the status line, the prompt box holding whatever menu is open, then the footer.

use crate::app::App;

use magi_tui::footer::{self, FooterData};
use magi_tui::metric;
use magi_tui::{fold, prompt, status, transcript};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;

/// Rows the chrome below the transcript occupies at its smallest: the prompt and the footer.
#[must_use]
pub fn chrome_rows() -> u16 {
    metric::prompt_min_rows() + metric::footer_rows()
}

/// Draw the live region, and say how many rows a surface could have had in it. The room is measured
/// where it is drawn — it depends on the footer, the rule and the prompt's wrapping — and the
/// session that grants rows has no terminal to measure.
pub fn draw(frame: &mut Frame<'_>, app: &mut App, footer_data: &FooterData) -> u16 {
    let area = frame.area();
    // Before anything is measured: an emptied prompt gets a new placeholder.
    app.settle_prompt();

    let rows = area.height;

    // The scan says what the session is doing, so it is chosen here rather than in the prompt. It
    // goes to whichever box is listening, since a pane owns the keyboard while it is open.
    let scan = if !app.connected {
        magi_tui::border::Scan::Off
    } else if app.is_busy() {
        magi_tui::border::Scan::Working
    } else if app.editor.is_blank() {
        magi_tui::border::Scan::Resting
    } else {
        magi_tui::border::Scan::Holding
    };
    let (scan, pane_scan) = if app.pane.is_some() {
        (magi_tui::border::Scan::Off, magi_tui::border::Scan::Focused)
    } else {
        (scan, magi_tui::border::Scan::Off)
    };
    // A row always sits between the transcript and the prompt, reserved either way: what changes is
    // whether it is blank or carries the rule, not how much room the transcript has.
    let scrolled = !app.scrollback.is_following();
    let more_rows = 1;

    // The menu goes inside the box, so there is no second region under it. It may not take the
    // whole screen: one row of transcript stays.
    let around = metric::footer_rows() + more_rows + 1;
    // The box wears the usage, cut to a third of the width first — the strip is reserved on every
    // row, so anything long here takes the whole prompt with it. See `magi_tui::corner`.
    let badge = app.corner.fitted(footer_data, area.width);
    let text_rows = prompt::text_rows(&app.editor, rows, area.width, &badge);
    let room = usize::from(rows.saturating_sub(around)).saturating_sub(text_rows + 3);
    // The same number the menu is cut to: a surface is drawn in the menu's slot.
    let granted = u16::try_from(room).unwrap_or(u16::MAX);
    // Keyed on what is open, so a permission ask after a model list is a second opening.
    app.landing
        .showing(app.overlay.as_ref().map(magi_tui::overlay::Overlay::key));
    // Rows a tool is holding go inside the box, where a picker, a permission and a completion
    // already go. It wins over an overlay, because a surface has the keyboard while it is up.
    let mut menu = match app.holding() {
        Some(held) if !held.drawn.is_empty() => {
            magi_tui::painted::lines(&held.drawn, ratatui::style::Style::default())
        }
        // Before its first frame, so the box does not jump open on nothing and again after.
        Some(held) => vec![ratatui::text::Line::from(ratatui::text::Span::styled(
            held.about.clone(),
            ratatui::style::Style::default().fg(magi_tui::colour::dim()),
        ))],
        None => app
            .overlay
            .as_ref()
            .map(|open| open.render(area.width.saturating_sub(metric::gutter() + 1)))
            .unwrap_or_default(),
    };
    menu.truncate(room);
    // While a turn runs the box says so in the placeholder's slot, and gets out of the way the
    // moment you type. The tease stays out of it.
    let effort = status::effort(app.status(), app.elapsed());
    // Drawn harder while what it opens is on screen, so the corner reads as currently pressed.
    let badge_open = app
        .pane
        .as_ref()
        .is_some_and(|open| open.title == app.corner.opens());
    let saying = if effort.is_empty() {
        magi_tui::tease::Saying {
            badge: &badge,
            badge_open,
            mode: app.modal.mode,
            ..app.tease.saying()
        }
    } else {
        magi_tui::tease::Saying {
            text: &effort,
            badge: &badge,
            badge_open,
            mode: app.modal.mode,
            ..Default::default()
        }
    };
    let prompt_lines = prompt::render(
        &app.editor,
        area.width,
        rows,
        app.scan_tick(),
        scan,
        &menu,
        saying,
    );
    let prompt_rows = u16::try_from(prompt_lines.lines.len())
        .unwrap_or(u16::MAX)
        .min(rows.saturating_sub(around - 1))
        .max(1);

    // Above the box: the transcript and its edge rules, and nothing else. Below it: one row, which
    // the footer draws.
    let [live_area, prompt_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(prompt_rows),
        Constraint::Length(metric::footer_rows()),
    ])
    .areas(area);

    // The whole transcript is ours and the reader's scroll position decides what shows. An empty
    // session draws nothing.
    let mut laid = transcript::laid_out(app.entries(), area.width, app.detail, &app.flipped);
    // After the layout and before the lines are handed over: the highlight is a fact about the
    // pointer, and re-rendering is what clears it.
    if let Some((line, column)) = app.hovering
        && let Some(under) = laid.lines.get_mut(line)
    {
        transcript::hovered(under, column);
    }
    app.owners = laid.owners;
    app.blocks = laid.blocks;
    app.scrollback.set_lines(laid.lines);
    // Each edge with something past it takes a row for its rule, so both sit against the text.
    let above = app.scrollback.hidden_above() > 0;
    let live_area = Rect {
        y: live_area.y + u16::from(above),
        height: live_area
            .height
            .saturating_sub(u16::from(above) + more_rows),
        ..live_area
    };
    let view = app.scrollback.view(live_area.height).to_vec();
    // Bottom-aligned: a transcript grows towards the prompt.
    let used = u16::try_from(view.len()).unwrap_or(live_area.height);
    let anchored = Rect {
        y: live_area.y + live_area.height.saturating_sub(used),
        height: used.min(live_area.height),
        ..live_area
    };
    // The rows the transcript landed on, which is what a click is measured against.
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
    if scrolled {
        frame.render_widget(
            Paragraph::new(status::more(area.width)),
            Rect {
                y: live_area.y + live_area.height,
                height: 1,
                ..live_area
            },
        );
    }

    // The UI picks the mood, not the agent. Anything open on the screen outranks whatever the agent
    // is doing: a permission ask arrives *during* a turn, so `is_busy()` first said `Working`.
    let mood = if !app.connected {
        magi_tui::beacon::Mood::Away
    } else if app
        .overlay
        .as_ref()
        .is_some_and(magi_tui::overlay::Overlay::is_completion)
    {
        magi_tui::beacon::Mood::Narrowing
    } else if app.overlay.is_some() {
        magi_tui::beacon::Mood::Asking
    } else if app.is_busy() {
        magi_tui::beacon::Mood::Working
    } else if app.editor.is_blank() {
        magi_tui::beacon::Mood::Resting
    } else {
        magi_tui::beacon::Mood::Holding
    };
    let mut status_line = status::working(&mut app.trace, mood, app.tick, area.width);
    if !app.connected {
        status_line.spans.extend(status::queued(app.queued));
    }
    // Where the tenant's rows and the usage badge actually landed, which is what a click on either
    // is measured against: the layout is the only thing that knows, and it knows it once.
    app.corner_rect = prompt_lines.badge.as_ref().map(|(row, columns)| Rect {
        x: prompt_area.x + columns.start,
        y: prompt_area.y + u16::try_from(*row).unwrap_or(u16::MAX),
        width: columns.end - columns.start,
        height: 1,
    });
    app.surface_rect = app.holding().map(|_| Rect {
        x: prompt_area.x + prompt::INSET,
        y: prompt_area.y + u16::try_from(prompt_lines.menu.start).unwrap_or(u16::MAX),
        width: area.width.saturating_sub(prompt::INSET + 1),
        height: u16::try_from(prompt_lines.menu.len()).unwrap_or(u16::MAX),
    });
    frame.render_widget(Paragraph::new(prompt_lines.lines), prompt_area);
    frame.render_widget(
        Paragraph::new(footer::render(footer_data, &status_line.spans, area.width)),
        footer_area,
    );

    // A float, over the finished screen and under nothing: it covers what it lands on rather than
    // reflowing the conversation. Its rect is recorded here, the only place that knows, so a press
    // outside it can close it.
    app.pane_rect = app.pane.as_ref().map(|_| magi_tui::pane::Pane::area(area));
    if let Some(open) = app.pane.as_mut() {
        let panel = magi_tui::pane::Pane::area(area);
        let page = magi_tui::pane::Pane::page(area);
        // Settled here because this is the only place that knows how tall the panel is.
        open.settle(page);

        // The heading is inside the frame, not in it: a border title breaks the rule for the word,
        // and a box with a gap in it reads as damaged. Light lines with arc corners, the only
        // weight Unicode gives rounded corners to.
        frame.render_widget(ratatui::widgets::Clear, panel);
        frame.render_widget(
            Paragraph::new(open.framed(panel.width, page, app.tick, pane_scan)),
            panel,
        );
    }

    // Last, over the finished screen: text that has not been drawn yet cannot arrive.
    if let Some(progress) = magi_tui::decrypt::progress() {
        magi_tui::decrypt::over(frame.buffer_mut(), area, progress);
    }
    // And again over a list that has just opened, on its rows alone.
    if let Some(progress) = app.landing.progress() {
        let rows = u16::try_from(menu.len()).unwrap_or(0);
        let top = prompt_area.y + prompt_area.height.saturating_sub(1 + rows);
        magi_tui::decrypt::over(
            frame.buffer_mut(),
            Rect {
                y: top,
                height: rows,
                ..prompt_area
            },
            progress,
        );
    }
    // The box only: a glitch mid tool result is indistinguishable from a tool that printed one.
    magi_tui::decrypt::flicker(frame.buffer_mut(), prompt_area);
    // Last of all: a highlight drawn before the effects is the one thing they could scribble on.
    if let Some(selection) = app.selection {
        magi_tui::select::over(frame.buffer_mut(), selection);
    }

    // A tenant that asked for the cursor gets it: while a surface has the keyboard, a caret parked
    // in the prompt points an IME and a screen reader at a box nothing is going into.
    if let Some((rect, at)) = app
        .surface_rect
        .zip(app.holding().and_then(|held| held.cursor))
    {
        frame.set_cursor_position((
            rect.x + at.col.min(rect.width.saturating_sub(1)),
            rect.y + at.row.min(rect.height.saturating_sub(1)),
        ));
        return granted;
    }
    place_hardware_cursor(frame, app, prompt_area, rows, &badge);
    granted
}

/// Park the terminal cursor on the same cell the inverted block is drawn on. The visible cursor is
/// the inverted cell; an IME window and a screen reader both follow the hardware one.
fn place_hardware_cursor(frame: &mut Frame<'_>, app: &App, area: Rect, rows: u16, badge: &str) {
    if area.height < 2 {
        return;
    }
    // Where the caret lands once the text is folded, not where it sits in a logical line.
    let (cursor_row, cursor_col) = fold::caret(&app.editor, area.width, badge);
    let visible = prompt::visible_rows(rows);
    let offset = cursor_row.saturating_sub(visible.saturating_sub(1));

    // Row 0 of the prompt area is the box's top edge, so the text begins one row down.
    let row = u16::try_from(cursor_row.saturating_sub(offset)).unwrap_or(0) + 1;
    if row >= area.height {
        return;
    }
    // And two columns in: the left bar, then the padding column.
    let col = u16::try_from(cursor_col)
        .unwrap_or(u16::MAX)
        .saturating_add(metric::gutter());
    frame.set_cursor_position((area.x + col.min(area.width.saturating_sub(1)), area.y + row));
}

/// The edge says the transcript continues, so "is this the end" is answered by looking.
#[cfg(test)]
mod continues_past_the_edge {
    use super::*;
    use magi_proto::{Cursor, HarnessEvent, MessageId};
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
            .draw(|frame| {
                draw(frame, &mut app, &footer);
            })
            .expect("draw");
        if lines > 0 {
            app.scrollback.scroll_up(lines);
        }
        terminal
            .draw(|frame| {
                draw(frame, &mut app, &footer);
            })
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
    fn the_lower_rule_sits_against_the_text_it_is_about() {
        // A rule with a blank row above it marks nothing.
        let rows = drawn(30, 10);
        let lower = *rules(&rows).last().expect("a rule below");
        let last = rows[..lower]
            .iter()
            .rposition(|row| !row.trim().is_empty())
            .expect("something above it");
        assert!(
            lower - last <= 2,
            "the rule drifted off the transcript: {rows:#?}"
        );
        let box_top = rows
            .iter()
            .position(|row| row.contains(char::from_u32(0x256D).expect("box corner")))
            .expect("the prompt is on screen");
        assert!(lower < box_top, "{rows:#?}");
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
        app.modal.open_command(&mut app.editor);
        app.editor.insert_str("mo");
        app.refresh_completion(&|_| Vec::new());
        assert!(app.overlay.is_some(), "the premise: `:mo` opens a menu");
        let footer = FooterData::default();
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).expect("test terminal");
        terminal
            .draw(|frame| {
                draw(frame, &mut app, &footer);
            })
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
        let menu: Vec<&String> = rows.iter().filter(|row| row.contains(":model")).collect();
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
            .position(|row| row.contains(":mo "))
            .expect("what was typed");
        let offered = rows
            .iter()
            .position(|row| row.contains(":model"))
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
            .position(|row| row.contains(":model"))
            .expect("what it offered");
        assert!(offered < bottom, "the menu fell out of the box: {rows:#?}");
    }
}

/// Where a surface landed, which is what a click on it is measured against — the one thing in the
/// surface path that can be silently wrong, since nothing about the picture shows an off-by-one.
#[cfg(test)]
mod where_the_rows_landed {
    use super::*;
    use magi_proto::surfacing::At;
    use magi_proto::tooling::{Role, Span};
    use magi_proto::{Cursor, HarnessEvent, ToolCallId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// A screen with a two-row surface open, drawing `MARK` on its second row.
    const MARK: &str = "second-row";

    fn played(cursor: Option<At>) -> (App, Terminal<TestBackend>) {
        let mut app = App::new();
        app.apply(HarnessEvent::Surfaced {
            cursor: Cursor(1),
            id: ToolCallId::new("s0"),
            tool: "dino".to_owned(),
            rows: 2,
            about: "a game".to_owned(),
        });
        app.apply(HarnessEvent::Drew {
            id: ToolCallId::new("s0"),
            lines: vec![
                vec![Span::new(Role::Text, "first")],
                vec![Span::new(Role::Text, MARK)],
            ],
            cursor,
        });
        let footer = FooterData::default();
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).expect("test terminal");
        terminal
            .draw(|frame| {
                draw(frame, &mut app, &footer);
            })
            .expect("draw");
        (app, terminal)
    }

    fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        terminal
            .backend()
            .buffer()
            .content
            .chunks(60)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    #[test]
    fn the_rect_names_the_rows_the_tenant_actually_drew_on() {
        // Checked against the frame rather than the arithmetic that produced it.
        let (app, terminal) = played(None);
        let drawn = rows(&terminal);
        let rect = app.surface_rect.expect("the rows were recorded");
        let mark = drawn
            .iter()
            .position(|row| row.contains(MARK))
            .expect("the tenant's second row is on screen");
        assert_eq!(usize::from(rect.y) + 1, mark, "{drawn:#?}");
        assert_eq!(rect.height, 2, "both rows, and no more");
        // And the column. Counted in characters, because the side of the box is three bytes.
        let from: String = drawn[mark].chars().skip(usize::from(rect.x)).collect();
        assert!(
            from.starts_with(MARK),
            "{:?} does not begin at column {}",
            drawn[mark],
            rect.x
        );
    }

    #[test]
    fn a_click_on_the_row_it_drew_arrives_as_that_row() {
        // The round trip: screen coordinates in, the tenant's own out.
        let (app, _drawn) = played(None);
        let rect = app.surface_rect.expect("the rows were recorded");
        assert_eq!(app.pointed_at(rect.y + 1, rect.x + 3), Some((1, 3)));
    }

    #[test]
    fn a_tenant_that_asked_for_the_caret_gets_it_in_its_own_rows() {
        let (app, mut terminal) = played(Some(At { row: 1, col: 4 }));
        let rect = app.surface_rect.expect("the rows were recorded");
        assert_eq!(
            terminal.get_cursor_position().expect("a cursor"),
            ratatui::layout::Position {
                x: rect.x + 4,
                y: rect.y + 1
            }
        );
    }

    #[test]
    fn a_surface_that_asked_for_nothing_leaves_the_caret_in_the_prompt() {
        // Nearly every one. A game wants nothing blinking in its picture.
        let (_, mut terminal) = played(None);
        let at = terminal.get_cursor_position().expect("a cursor");
        // Row 1 of the box, which is the text row -- above the divider, and so above the surface.
        let rect = played(None).0.surface_rect.expect("recorded");
        assert!(at.y < rect.y, "the caret went into the tenant's rows");
    }
}
