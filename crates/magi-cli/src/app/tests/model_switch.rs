use super::super::*;

fn info(name: &str) -> magi_proto::ModelInfo {
    magi_proto::ModelInfo {
        name: name.to_owned(),
        context_window: 100_000,
    }
}

fn said(app: &App) -> Vec<String> {
    app.entries()
        .iter()
        .filter_map(|e| match e {
            Entry::Notice { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_switch_mid_conversation_is_written_down() {
    // Which model answered is part of the record; a switch that moves two dim words in
    // the footer leaves no mark where a reader is actually looking.
    let mut app = App::new();
    app.apply(HarnessEvent::UserMessage {
        images: Vec::new(),
        cursor: Cursor(1),
        id: MessageId::new("u1"),
        text: "hello".into(),
    });
    app.apply(HarnessEvent::ModelChanged {
        cursor: Cursor(2),
        model: Some(info("openrouter/anthropic/claude-sonnet-4.5")),
        thinking: None,
    });
    assert!(
        said(&app).iter().any(|t| t.contains("claude-sonnet-4.5")),
        "{:?}",
        said(&app)
    );
}

#[test]
fn the_model_the_session_opened_with_is_not_an_announcement() {
    // Every attach reports the model. Announcing that is a line of noise on every start.
    let mut app = App::new();
    app.apply(HarnessEvent::ModelChanged {
        cursor: Cursor(1),
        model: Some(info("m")),
        thinking: None,
    });
    assert!(said(&app).is_empty(), "{:?}", said(&app));
}

#[test]
fn re_reporting_the_same_model_says_nothing() {
    // A reconnect replays the model; it has not changed.
    let mut app = App::new();
    app.apply(HarnessEvent::UserMessage {
        images: Vec::new(),
        cursor: Cursor(1),
        id: MessageId::new("u1"),
        text: "hello".into(),
    });
    app.apply(HarnessEvent::ModelChanged {
        cursor: Cursor(2),
        model: Some(info("m")),
        thinking: None,
    });
    let after_first = said(&app).len();
    app.apply(HarnessEvent::ModelChanged {
        cursor: Cursor(3),
        model: Some(info("m")),
        thinking: None,
    });
    assert_eq!(said(&app).len(), after_first);
}
