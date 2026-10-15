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
        id: ToolCallId::new("t2"),
        tool: "dino".into(),
        rows: 2,
        about: "a game".into(),
        place: magi_proto::tooling::Place::Block,
    });
    app.apply(HarnessEvent::Drew {
        id: ToolCallId::new("t2"),
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
