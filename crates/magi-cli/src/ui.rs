//! Drawing the screen.
//!
//! magi owns every cell: the transcript is a buffer it keeps and scrolls, not lines handed to
//! the terminal. That is what the wheel, the scroll keys, the edge rule and clicking a block
//! open are all written against.
//!
//! Below the transcript: the status line, then the prompt box — which holds whatever menu is
//! open — then the footer. The two edge rules are the transcript's own, drawn against its top
//! and bottom when it runs past them.

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

/// Draw the live region.
pub fn draw(frame: &mut Frame<'_>, app: &mut App, footer_data: &FooterData) {
    let area = frame.area();
    // Before anything is measured: an emptied prompt gets a new placeholder, and the one it
    // gets is what this frame draws.
    app.settle_prompt();

    let rows = area.height;

    // The scan says what the session is doing, which is why it is chosen here rather than in the
    // prompt: this is the only place that knows about the turn as well as the text.
    let scan = if !app.connected {
        magi_tui::border::Scan::Off
    } else if app.is_busy() {
        magi_tui::border::Scan::Working
    } else if app.editor.is_blank() {
        magi_tui::border::Scan::Resting
    } else {
        magi_tui::border::Scan::Holding
    };
    // **A row always sits between the transcript and the prompt.** It was taken only when there
    // was something to say in it, so following the newest output put the last line of the
    // conversation directly against the box you answer in, and scrolling away pushed everything
    // up by one. Reserved either way: what changes is whether the row is blank or carries the
    // rule, not how much room the transcript has.
    //
    // "Not following" is exactly "you have scrolled away from the newest output", which is when
    // the rule is worth drawing. It says only that the transcript continues.
    let scrolled = !app.scrollback.is_following();
    let more_rows = 1;

    // The menu goes inside the box, so the box is as tall as the two of them together and there
    // is no second region under it. What it may not do is take the whole screen: one row of
    // transcript stays, or a list opened mid-turn hides the turn it is about.
    let around = metric::footer_rows() + more_rows + 1;
    // The box wears the usage: it is the number you want while you are deciding what to send,
    // and the box is where you are looking when you decide. Cut to a third of the width first --
    // the strip is reserved on every row, so anything long here takes the whole prompt with it.
    let badge = footer::usage(footer_data);
    let badge = if badge.chars().count() <= usize::from(area.width) / 3 {
        badge
    } else {
        // The window is the part that matters when there is not room for all of it: the totals
        // are a tally and this one is a limit you are walking towards.
        footer::usage(&footer::FooterData {
            input_tokens: 0,
            output_tokens: 0,
            ..footer_data.clone()
        })
    };
    let text_rows = prompt::text_rows(&app.editor, rows, area.width, &badge);
    let room = usize::from(rows.saturating_sub(around)).saturating_sub(text_rows + 3);
    // Keyed on what is open, so a permission ask after a model list is a second opening while
    // either one narrowing under a query is still the first.
    app.landing
        .showing(app.overlay.as_ref().map(magi_tui::overlay::Overlay::key));
    // **Rows a tool is holding go where every other choice goes: inside the box.** A picker, a
    // permission, a completion and a surface are the same thing to a reader — something asking
    // for the keyboard — and they belong in the one place already reserved for that. Given a
    // region of its own above the prompt, a surface opened a band in the middle of the screen and
    // was the only control here that did not appear where the others do.
    //
    // It wins over an overlay, because a surface has the keyboard while it is up: a list left
    // underneath would be one nothing could reach.
    let mut menu = match app.holding() {
        Some(held) if !held.drawn.is_empty() => {
            magi_tui::painted::lines(&held.drawn, ratatui::style::Style::default())
        }
        // Before its first frame. Otherwise the box would jump open on nothing, then again when
        // the tenant drew.
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
    // While a turn runs the box says so, in the placeholder's slot: it is where you are looking
    // and it is the one place with room for a sentence. It gets out of the way the moment you
    // type, and typing during a turn is allowed and always was. The tease stays out of it --
    // a box writing to itself while the agent works is two things claiming the same line.
    let effort = status::effort(app.status(), app.elapsed());
    let saying = if effort.is_empty() {
        magi_tui::tease::Saying {
            badge: &badge,
            mode: app.modal.mode,
            ..app.tease.saying()
        }
    } else {
        magi_tui::tease::Saying {
            text: &effort,
            badge: &badge,
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
    // the footer draws — what the agent is doing, then usage, then the model. The status line had
    // a row of its own above the box, which is a row of chrome for one word, in the one place
    // where nothing should stand between what was said and where you answer it.
    let [live_area, prompt_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(prompt_rows),
        Constraint::Length(metric::footer_rows()),
    ])
    .areas(area);

    // The whole transcript is ours and the reader's scroll position decides what shows.
    //
    // An empty session draws nothing. There was a greeting here — a name, the model, the
    // directory and four key hints — and it was right for somebody meeting the thing for the
    // first time and wrong for everybody after that. The prompt's own placeholder still names
    // `:`, which is the one line of it worth keeping.
    let mut laid = transcript::laid_out(app.entries(), area.width, app.detail, &app.flipped);
    // After the layout and before the lines are handed over, because the highlight is a fact
    // about the pointer rather than about the transcript: it must not survive into the next
    // frame on its own, and re-rendering is what clears it.
    if let Some((line, column)) = app.hovering
        && let Some(under) = laid.lines.get_mut(line)
    {
        transcript::hovered(under, column);
    }
    app.owners = laid.owners;
    app.blocks = laid.blocks;
    app.scrollback.set_lines(laid.lines);
    // Each edge that has something past it takes a row out of the transcript for its rule, so
    // both sit against the text rather than out in the chrome.
    let above = app.scrollback.hidden_above() > 0;
    let live_area = Rect {
        y: live_area.y + u16::from(above),
        height: live_area
            .height
            .saturating_sub(u16::from(above) + more_rows),
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

    // The UI picks the mood, not the agent: a list being open and the prompt having text in it
    // are both states worth showing and neither is anything the daemon reports.
    // Anything open on the screen outranks whatever the agent is doing, because it is the thing
    // holding everything up. A permission ask arrives *during* a turn, so asking `is_busy()`
    // first meant the one moment magi is waiting on you was the one moment it said `Working`.
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
    // Where the tenant's rows actually landed, which is what a click on them is measured against.
    // Recorded here for the same reason `live_rows` is: the layout is the only thing that knows,
    // and it knows it only once. Cleared when nothing is holding them, so a pointer over a picker
    // is not translated into coordinates for a surface that closed.
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

    // Last, over the finished screen: the effect is about the text arriving, and text that has
    // not been drawn yet cannot arrive. Off unless `magi.ui.decrypt_ms` says otherwise.
    if let Some(progress) = magi_tui::decrypt::progress() {
        magi_tui::decrypt::over(frame.buffer_mut(), area, progress);
    }
    // And again over a list that has just opened, on its rows alone. A model list, a permission
    // ask and a session picker all arrive the same way the screen did — the box is already
    // there, and the choices land into it.
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
    // The box only. A glitch in the middle of a tool result is indistinguishable from a tool
    // that printed a glitch.
    magi_tui::decrypt::flicker(frame.buffer_mut(), prompt_area);
    // Last of all, over everything: a selection is about what is on the screen, and a highlight
    // drawn before the effects would be the one thing they could scribble on.
    if let Some(selection) = app.selection {
        magi_tui::select::over(frame.buffer_mut(), selection);
    }

    // **A tenant that asked for the cursor gets it.** While a surface holds the keyboard the
    // prompt is not where you are typing, so leaving the caret parked in it points an IME and a
    // screen reader at a box nothing is going into. Only when it asks: a game wants nothing
    // blinking in its picture, and that is nearly every surface.
    if let Some((rect, at)) = app.surface_rect.zip(app.holding().and_then(|held| held.cursor)) {
        frame.set_cursor_position((
            rect.x + at.col.min(rect.width.saturating_sub(1)),
            rect.y + at.row.min(rect.height.saturating_sub(1)),
        ));
        return;
    }
    place_hardware_cursor(frame, app, prompt_area, rows, &badge);
}

/// Park the terminal cursor on the same cell the inverted block is drawn on.
///
/// The visible cursor is the inverted cell; this is for the terminal's own benefit â an IME
/// candidate window and a screen reader both follow the hardware cursor, not the colours.
fn place_hardware_cursor(frame: &mut Frame<'_>, app: &App, area: Rect, rows: u16, badge: &str) {
    if area.height < 2 {
        return;
    }
    // Where the caret lands once the text is folded, not where it sits in a logical line. A long
    // line is several rows now, and the two answers differ by however many times it wrapped.
    let (cursor_row, cursor_col) = fold::caret(&app.editor, area.width, badge);
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
    fn the_lower_rule_sits_against_the_text_it_is_about() {
        // It had a row of its own down by the prompt, with the status line between it and the
        // transcript — and a rule with a blank row above it marks nothing.
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

/// Where a surface landed, which is what a click on it is measured against.
///
/// The one thing in the whole surface path that can be silently wrong: an off-by-one here is a
/// game that jumps when you click one row above it, and nothing about the picture says so.
#[cfg(test)]
mod where_the_rows_landed {
    use super::*;
    use magi_proto::tooling::{At, Role, Span};
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
            .draw(|frame| draw(frame, &mut app, &footer))
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
        // Checked against the frame rather than against the arithmetic that produced it. The two
        // agreeing is the whole claim: a click at screen row `rect.y + 1` is the tenant's row 1.
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
