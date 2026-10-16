//! The transcript is drawn from the stack: blocks that did not change are not drawn again, and
//! what the pointer lights up is drawn over the screen rather than kept in a block.

use super::*;
use magi_proto::{Cursor, HarnessEvent, MessageId, ToolCallId, ToolResult};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

const WIDTH: u16 = 60;

fn conversation() -> App {
    let mut app = App::new();
    app.apply(HarnessEvent::UserMessage {
        images: Vec::new(),
        cursor: Cursor(1),
        id: MessageId::new("u1"),
        text: "list the files".into(),
    });
    app.apply(HarnessEvent::ToolCallStarted {
        cursor: Cursor(2),
        id: ToolCallId::new("t1"),
        name: "shell".into(),
        args: r#"{"command":"ls"}"#.into(),
    });
    app.apply(HarnessEvent::ToolCallEnded {
        cursor: Cursor(3),
        id: ToolCallId::new("t1"),
        result: ToolResult {
            images: Vec::new(),
            output: "a.rs\nb.rs".into(),
            is_error: false,
            shown: None,
        },
    });
    app
}

fn drawn(app: &mut App, terminal: &mut Terminal<TestBackend>) {
    terminal
        .draw(|frame| {
            draw(frame, app, &FooterData::default());
        })
        .expect("draw");
}

/// Where the fold handle's glyph is on screen.
fn handle(terminal: &Terminal<TestBackend>) -> (u16, u16) {
    let buffer = terminal.backend().buffer();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            if buffer[(x, y)].symbol() == magi_tui::glyph::expand() {
                return (y, x);
            }
        }
    }
    panic!("no handle on screen");
}

#[test]
fn a_frame_with_nothing_new_draws_no_block_again() {
    let mut app = conversation();
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, 20)).expect("terminal");
    drawn(&mut app, &mut terminal);
    assert_eq!(app.stack.drawn(), 2, "the message and the call");
    drawn(&mut app, &mut terminal);
    assert_eq!(app.stack.drawn(), 0);
}

#[test]
fn a_lit_chip_is_drawn_on_screen_and_not_kept_in_its_block() {
    let mut app = conversation();
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, 20)).expect("terminal");
    drawn(&mut app, &mut terminal);
    let (row, column) = handle(&terminal);
    let line = app.scrollback.hidden_above() + usize::from(row - app.live_rows.start);
    let kept = app.stack.line(line).cloned().expect("the header");

    assert!(app.hover_at(row, column), "the pointer is on the handle");
    drawn(&mut app, &mut terminal);
    let lit = terminal.backend().buffer()[(column, row)].modifier;
    assert!(
        !lit.contains(Modifier::REVERSED),
        "the chip is inverted on screen"
    );
    assert_eq!(
        app.stack.line(line),
        Some(&kept),
        "and the block is as it was"
    );
    assert_eq!(app.stack.drawn(), 0, "nothing was drawn again to light it");
}

/// A tool drawing in its own call's block, with one frame drawn.
fn playing() -> App {
    let mut app = conversation();
    app.apply(HarnessEvent::ToolCallStarted {
        cursor: Cursor(4),
        id: ToolCallId::new("t2"),
        name: "games".into(),
        args: "{}".into(),
    });
    app.apply(HarnessEvent::Surfaced {
        cursor: Cursor(5),
        id: ToolCallId::new("s0"),
        tool: "dino".into(),
        rows: 2,
        about: "a game".into(),
        place: magi_proto::tooling::Place::Block,
        call: Some(ToolCallId::new("t2")),
        detach: false,
    });
    app.apply(HarnessEvent::Drew {
        id: ToolCallId::new("s0"),
        lines: vec![
            vec![magi_proto::tooling::Span::new(
                magi_proto::tooling::Role::Text,
                "the dinosaur runs",
            )],
            vec![magi_proto::tooling::Span::new(
                magi_proto::tooling::Role::Text,
                "over a cactus",
            )],
        ],
        cursor: None,
    });
    app
}

fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[test]
fn a_surface_in_its_own_block_is_drawn_in_the_transcript() {
    let mut app = playing();
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, 24)).expect("terminal");
    drawn(&mut app, &mut terminal);
    let shown = rows(&terminal);
    let header = shown
        .iter()
        .position(|row| row.contains("[ dino ]"))
        .expect("the live block's header");
    assert!(
        shown[header + 1].contains("the dinosaur runs"),
        "{shown:#?}"
    );
    assert!(shown[header + 2].contains("over a cactus"), "{shown:#?}");
    let prompt = shown
        .iter()
        .position(|row| row.starts_with('╭'))
        .expect("the prompt");
    assert!(header < prompt, "{shown:#?}");
    assert!(
        !shown[prompt..].iter().any(|row| row.contains("dinosaur")),
        "and not in the prompt box: {shown:#?}"
    );
}

#[test]
fn the_pointer_over_a_block_surface_arrives_in_its_own_coordinates() {
    let mut app = playing();
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, 24)).expect("terminal");
    drawn(&mut app, &mut terminal);
    let shown = rows(&terminal);
    let first = u16::try_from(
        shown
            .iter()
            .position(|row| row.contains("the dinosaur runs"))
            .expect("the frame"),
    )
    .expect("short");
    let left = magi_tui::transcript::body_left();
    assert_eq!(app.pointed_at(first, left), Some((0, 0)));
    assert_eq!(app.pointed_at(first + 1, left + 3), Some((1, 3)));
    assert_eq!(
        app.pointed_at(first - 1, left),
        None,
        "the header is not the tool's"
    );
}

#[test]
fn a_block_surface_scrolled_out_of_view_hears_no_pointer() {
    let mut app = playing();
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, 24)).expect("terminal");
    drawn(&mut app, &mut terminal);
    app.scrollback.to_top();
    for n in 0..20 {
        app.apply(HarnessEvent::UserMessage {
            images: Vec::new(),
            cursor: Cursor(10 + n),
            id: MessageId::new(format!("later{n}")),
            text: "more".into(),
        });
    }
    app.scrollback.to_bottom();
    drawn(&mut app, &mut terminal);
    assert_eq!(app.surface_rect, None);
}

/// A call whose tool left `htop` running in its block, drawn with a usage badge to stand beside.
fn left_running() -> (App, Terminal<TestBackend>) {
    let mut app = conversation();
    app.apply(HarnessEvent::ToolCallStarted {
        cursor: Cursor(4),
        id: ToolCallId::new("t3"),
        name: "screen".into(),
        args: r#"{"command":"htop","title":"system monitor"}"#.into(),
    });
    app.apply(HarnessEvent::Surfaced {
        cursor: Cursor(5),
        id: ToolCallId::new("s1"),
        tool: "screen".into(),
        rows: 2,
        about: "system monitor".into(),
        place: magi_proto::tooling::Place::Block,
        call: Some(ToolCallId::new("t3")),
        detach: true,
    });
    app.apply(HarnessEvent::Drew {
        id: ToolCallId::new("s1"),
        lines: vec![vec![magi_proto::tooling::Span::new(
            magi_proto::tooling::Role::Text,
            "load average 0.42",
        )]],
        cursor: None,
    });
    let terminal = Terminal::new(TestBackend::new(WIDTH, 30)).expect("terminal");
    (app, terminal)
}

fn with_usage(app: &mut App, terminal: &mut Terminal<TestBackend>) {
    let footer = FooterData {
        context_window: 200_000,
        context_percent: Some(12.0),
        ..FooterData::default()
    };
    terminal
        .draw(|frame| {
            draw(frame, app, &footer);
        })
        .expect("draw");
}

#[test]
fn the_prompt_counts_the_blocks_left_running_beside_its_usage() {
    let (mut app, mut terminal) = left_running();
    with_usage(&mut app, &mut terminal);
    let shown = rows(&terminal);
    let badge = shown
        .iter()
        .find(|row| row.contains("12%"))
        .expect("the usage badge");
    assert!(badge.contains(" 1   12% "), "{badge:?}");
    let chip = app.live_rect.expect("the running chip");
    let usage = app.corner_rect.expect("the usage chip");
    assert!(chip.x + chip.width < usage.x, "two chips, side by side");
    assert!(app.holding().is_none(), "the prompt keeps the keys");
}

#[test]
fn a_pinned_block_sits_above_the_prompt_and_its_place_says_so() {
    let (mut app, mut terminal) = left_running();
    app.toggle_pin(&ToolCallId::new("s1"));
    with_usage(&mut app, &mut terminal);
    let shown = rows(&terminal);
    let prompt = shown
        .iter()
        .position(|row| row.starts_with('╭'))
        .expect("the prompt");
    assert!(shown[prompt - 2].contains("[ screen ]"), "{shown:#?}");
    assert!(shown[prompt - 1].contains("system monitor"), "{shown:#?}");
    assert!(
        shown[..prompt - 2]
            .iter()
            .any(|row| row.contains("pinned above the prompt")),
        "{shown:#?}"
    );
    assert!(
        !shown.iter().any(|row| row.contains("load average")),
        "folded, its frame is drawn nowhere: {shown:#?}"
    );
}

#[test]
fn an_open_pinned_block_draws_its_frame_above_the_prompt_alone() {
    let (mut app, mut terminal) = left_running();
    let id = ToolCallId::new("s1");
    app.toggle_pin(&id);
    app.toggle_docked(&id);
    with_usage(&mut app, &mut terminal);
    let shown = rows(&terminal);
    let prompt = shown
        .iter()
        .position(|row| row.starts_with('╭'))
        .expect("the prompt");
    let frames: Vec<usize> = shown
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains("load average"))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(frames.len(), 1, "drawn in one place: {shown:#?}");
    assert!(frames[0] < prompt && frames[0] + 3 >= prompt, "{shown:#?}");
    let at = app.surface_rect.expect("it has the keyboard");
    assert_eq!(usize::from(at.y), frames[0]);
}

#[test]
fn a_chip_on_a_pinned_block_lights_under_the_pointer() {
    let (mut app, mut terminal) = left_running();
    app.toggle_pin(&ToolCallId::new("s1"));
    with_usage(&mut app, &mut terminal);
    let row = app.docked.first().expect("docked").1.y;
    let buffer = terminal.backend().buffer();
    let column = (0..WIDTH)
        .find(|x| buffer[(*x, row)].symbol() == magi_tui::glyph::expand())
        .expect("the dock's handle");
    assert!(
        app.hover_dock(row, column, WIDTH),
        "the pointer is on its handle"
    );
    assert!(
        !app.hover_dock(row, column, WIDTH),
        "and moving within it changes nothing"
    );
    let before = terminal.backend().buffer()[(column, row)].modifier;
    with_usage(&mut app, &mut terminal);
    let after = terminal.backend().buffer()[(column, row)].modifier;
    assert_ne!(before, after, "the chip is inverted on screen");
    assert!(app.hover_dock(row, 0, WIDTH), "off the chip, it goes dark");
}
