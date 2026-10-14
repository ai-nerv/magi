//! What a hostile string and a terminal too small to hold the chrome do to a frame.

use super::*;
use magi_proto::permit::{Action, Scope};
use magi_proto::{Cursor, HarnessEvent, MessageId, ToolCallId};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn drawn(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            draw(frame, app, &FooterData::default());
        })
        .expect("draw");
    terminal
}

fn cells(terminal: &Terminal<TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

fn controls(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c))
}

#[test]
fn an_escape_in_model_text_is_shown_and_never_sent() {
    let mut app = App::new();
    app.apply(HarnessEvent::AssistantStarted {
        cursor: Cursor(1),
        id: MessageId::new("a1"),
    });
    app.apply(HarnessEvent::AssistantDelta {
        cursor: Cursor(2),
        id: MessageId::new("a1"),
        text: "look \x1b]52;c;ZXZpbA==\x07 and \x1b]0;owned\x07 \u{9b}2J done".into(),
        thinking: String::new(),
    });
    let shown = cells(&drawn(&mut app, 80, 20));
    assert!(!controls(&shown), "a control reached the frame: {shown:?}");
    assert!(shown.contains('\u{241b}'), "and it is shown: {shown:?}");
}

#[test]
fn a_permission_subject_shows_what_would_have_hidden_it() {
    let mut app = App::new();
    let command = "rm -rf ~ \r\x1b[2K git status";
    app.apply(HarnessEvent::PermissionAsked {
        cursor: Cursor(1),
        id: ToolCallId::new("t1"),
        tool: "shell".into(),
        action: Action::Run {
            command: command.into(),
            program: "rm".into(),
        },
        offers: vec![Scope::Once],
        advice: None,
    });
    let shown = cells(&drawn(&mut app, 100, 24));
    assert!(!controls(&shown), "{shown:?}");
    assert!(
        shown.contains("rm -rf ~"),
        "the hidden part is on screen: {shown:?}"
    );
    assert!(shown.contains("\u{240d}\u{241b}[2K"), "{shown:?}");
}

#[test]
fn no_rows_or_one_while_scrolled_draws_and_does_not_panic() {
    for (width, height) in [(60, 0), (60, 1), (60, 2), (0, 16), (1, 1), (3, 3)] {
        let mut app = App::new();
        for n in 0..40 {
            app.apply(HarnessEvent::UserMessage {
                cursor: Cursor(n + 1),
                id: MessageId::new(format!("u{n}")),
                text: format!("question number {n}"),
            });
        }
        let mut terminal = drawn(&mut app, 60, 16);
        app.scrollback.scroll_up(7);
        app.selection = Some({
            let mut chosen = magi_tui::select::Selection::begin(3, 2);
            chosen.drag_to(12, 40);
            chosen
        });
        terminal.backend_mut().resize(width, height);
        terminal
            .draw(|frame| {
                draw(frame, &mut app, &FooterData::default());
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let area = buffer.area;
        let _ = magi_tui::select::text(&buffer, app.selection.expect("set"), area);
    }
}
