//! Screen-level tests.
//!
//! Pi's differential renderer is only testable because its harness wraps a real terminal
//! emulator; `TestBackend` is the same idea. These assert on what a user sees, not on what a
//! function returned.

use axum_proto::{
    AgentStatus, Cursor, Entry, HarnessEvent, MessageId, StopReason, ToolCallId, ToolResult,
};
use axum_tui::footer::FooterData;
use axum_tui::transcript;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// Everything on screen, one string per row, trailing blanks trimmed.
fn screen(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn render_entries(entries: &[Entry], width: u16) -> Vec<String> {
    let lines = transcript::render(entries, width, transcript::Detail::Preview);
    let height = lines.len() as u16;
    let mut terminal =
        Terminal::new(TestBackend::new(width, height.max(1))).expect("test terminal");
    terminal
        .draw(|frame| {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(lines.clone()),
                frame.area(),
            );
        })
        .expect("draw");
    screen(&terminal)
}

#[test]
fn a_conversation_renders_the_way_pi_lays_it_out() {
    let entries = vec![
        Entry::User {
            id: MessageId::new("m1"),
            text: "run the tests".into(),
        },
        Entry::Assistant {
            id: MessageId::new("a1"),
            text: "Running them now.".into(),
            thinking: String::new(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            signatures: axum_proto::Signatures::default(),
            usage: axum_proto::Usage::default(),
        },
        Entry::Tool {
            id: ToolCallId::new("t1"),
            name: "bash".into(),
            args: r#"{"command": "cargo test"}"#.into(),
            result: Some(ToolResult {
                output: "test result: ok. 42 passed".into(),
                is_error: false,
            }),
            thought_signature: None,
        },
    ];

    let rendered = render_entries(&entries, 44);
    assert_eq!(
        rendered,
        vec![
            "",                   // user box: top padding
            " run the tests",     // user box: body
            "",                   // user box: bottom padding
            "",                   // assistant: leading blank
            " Running them now.", // assistant: body, indented
            "",                   // tool box: top padding
            // tool box: the name reversed out as a label, then its arguments, then the
            // handle that opens the block, held against the right edge.
            "  bash  cargo test                        »",
            " test result: ok. 42 passed", // tool box: output
            "",                            // tool box: bottom padding
        ]
    );
}

#[test]
fn a_failed_tool_still_shows_its_output() {
    let entries = vec![Entry::Tool {
        id: ToolCallId::new("t1"),
        name: "bash".into(),
        args: "{}".into(),
        result: Some(ToolResult {
            output: "error: could not compile".into(),
            is_error: true,
        }),
        thought_signature: None,
    }];
    let rendered = render_entries(&entries, 40);
    assert!(
        rendered.iter().any(|l| l.contains("could not compile")),
        "{rendered:?}"
    );
}

#[test]
fn thinking_renders_above_the_response() {
    let entries = vec![Entry::Assistant {
        id: MessageId::new("a1"),
        text: "Here is the answer.".into(),
        thinking: "Let me consider this.".into(),
        stop_reason: Some(StopReason::EndTurn),
        error: None,
        signatures: axum_proto::Signatures::default(),
        usage: axum_proto::Usage::default(),
    }];
    let rendered = render_entries(&entries, 40);
    let thinking = rendered
        .iter()
        .position(|l| l.contains("consider"))
        .expect("thinking row");
    let answer = rendered
        .iter()
        .position(|l| l.contains("answer"))
        .expect("answer row");
    assert!(thinking < answer, "{rendered:?}");
}

#[test]
fn every_row_of_a_user_box_spans_the_full_width() {
    let entry = Entry::User {
        id: MessageId::new("m1"),
        text: "short".into(),
    };
    for line in transcript::entry_lines(&entry, 30, transcript::Detail::Preview) {
        let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(width, 30, "a background row must fill the width");
    }
}

#[test]
fn the_recorded_sample_replays_into_a_transcript() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/recordings/hello.jsonl"
    ))
    .expect("sample recording");

    let mut entries: Vec<Entry> = Vec::new();
    let mut status = AgentStatus::Idle;
    let mut cursor = Cursor::ZERO;

    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let event: HarnessEvent = serde_json::from_str(line).expect("event parses");
        assert!(event.cursor() > cursor, "cursors ascend: {line}");
        cursor = event.cursor();

        match event {
            HarnessEvent::UserMessage { id, text, .. } => entries.push(Entry::User { id, text }),
            HarnessEvent::AssistantStarted { id, .. } => entries.push(Entry::Assistant {
                id,
                text: String::new(),
                thinking: String::new(),
                stop_reason: None,
                error: None,
                signatures: axum_proto::Signatures::default(),
                usage: axum_proto::Usage::default(),
            }),
            HarnessEvent::AssistantDelta { text, thinking, .. } => {
                if let Some(Entry::Assistant {
                    text: body,
                    thinking: reasoning,
                    ..
                }) = entries.last_mut()
                {
                    body.push_str(&text);
                    reasoning.push_str(&thinking);
                }
            }
            HarnessEvent::ToolCallStarted { id, name, args, .. } => entries.push(Entry::Tool {
                id,
                name,
                args,
                result: None,
                thought_signature: None,
            }),
            HarnessEvent::ToolCallEnded { result, .. } => {
                if let Some(Entry::Tool { result: slot, .. }) = entries.last_mut() {
                    *slot = Some(result);
                }
            }
            HarnessEvent::StatusChanged { status: s, .. } => status = s,
            _ => {}
        }
    }

    assert_eq!(status, AgentStatus::Idle, "the sample ends idle");
    assert_eq!(entries.len(), 7, "1 user, 3 assistant, 3 tool");

    let rendered = render_entries(&entries, 76);
    assert!(rendered.iter().any(|l| l.contains("torn-tail")));
    assert!(rendered.iter().any(|l| l.contains("• truncate a trailing")));
    assert!(rendered.iter().any(|l| l.contains("2 passed")));
}

#[test]
fn a_footer_row_is_exactly_the_terminal_width() {
    let data = FooterData {
        cwd: "~/src/axum".into(),
        branch: Some("develop".into()),
        input_tokens: 12_500,
        output_tokens: 900,
        context_percent: Some(6.2),
        context_window: 200_000,
        model: "claude-opus-5".into(),
        mode: "inline",
        mouse_held: false,
    };
    let lines = axum_tui::footer::render(&data, 70);
    let stats: usize = lines[0]
        .spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum();
    assert_eq!(stats, 70);
}

/// Not an assertion: prints the sample transcript so the layout can be eyeballed.
/// Run with `cargo test -p axum-cli -- --ignored --nocapture visual`.
#[test]
#[ignore = "visual inspection only"]
fn visual() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/recordings/hello.jsonl"
    ))
    .expect("sample recording");
    let mut entries: Vec<Entry> = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let event: HarnessEvent = serde_json::from_str(line).expect("event");
        match event {
            HarnessEvent::UserMessage { id, text, .. } => entries.push(Entry::User { id, text }),
            HarnessEvent::AssistantStarted { id, .. } => entries.push(Entry::Assistant {
                id,
                text: String::new(),
                thinking: String::new(),
                stop_reason: None,
                error: None,
                signatures: axum_proto::Signatures::default(),
                usage: axum_proto::Usage::default(),
            }),
            HarnessEvent::AssistantDelta { text, thinking, .. } => {
                if let Some(Entry::Assistant {
                    text: b,
                    thinking: r,
                    ..
                }) = entries.last_mut()
                {
                    b.push_str(&text);
                    r.push_str(&thinking);
                }
            }
            HarnessEvent::ToolCallStarted { id, name, args, .. } => entries.push(Entry::Tool {
                id,
                name,
                args,
                result: None,
                thought_signature: None,
            }),
            HarnessEvent::ToolCallEnded { result, .. } => {
                if let Some(Entry::Tool { result: slot, .. }) = entries.last_mut() {
                    *slot = Some(result);
                }
            }
            _ => {}
        }
    }
    for row in render_entries(&entries, 76) {
        println!("|{row}");
    }
}
