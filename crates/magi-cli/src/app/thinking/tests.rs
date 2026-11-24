use super::*;
use magi_proto::{Cursor, HarnessEvent, ModelChoice, ModelInfo, UiCommand};

fn app(levels: Option<Vec<&str>>) -> App {
    let mut app = App::new();
    app.model = Some(ModelInfo {
        name: "p/m".into(),
        context_window: 1000,
    });
    app.model_reasons = true;
    app.choices = vec![ModelChoice {
        display_name: None,
        provider: None,
        name: "p/m".into(),
        context_window: 1000,
        requirement: String::new(),
        pricing: None,
        popularity_rank: None,
        vision: false,
        wants_vars: Vec::new(),
        reasoning: true,
        subscription: false,
        capabilities: Some(magi_model::Capabilities {
            thinking: Some(vec!["published".into()]),
            thinking_levels: levels.map(|levels| levels.into_iter().map(str::to_owned).collect()),
            ..Default::default()
        }),
    }];
    app
}

#[test]
fn picker_and_steps_only_offer_declared_controls_in_canonical_order() {
    let mut app = app(Some(vec!["xhigh", "low"]));
    app.thinking = "low".into();
    app.open_thinking_picker();
    let picker = app
        .overlay
        .as_ref()
        .and_then(|overlay| overlay.list())
        .expect("picker");
    let ready = picker
        .choices
        .iter()
        .filter(|choice| choice.ready)
        .map(|choice| choice.value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ready, ["low", "xhigh"]);
    let display = magi_tui::picker::render(picker, 100)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        display.contains("xhigh") && display.contains("not selectable"),
        "{display}"
    );
    assert_eq!(app.thinking_step(true, false), Some("xhigh"));
    assert_eq!(app.thinking_step(false, false), None);
    app.thinking = "xhigh".into();
    assert_eq!(app.thinking_step(true, true), Some("low"));
    app.picking = Some(Picking::Thinking);
    assert!(app.chose("max".into()).is_none());
    app.picking = Some(Picking::Thinking);
    assert!(
        matches!(app.chose("xhigh".into()), Some(UiCommand::SetThinking { level }) if level=="xhigh")
    );
}

#[test]
fn metadata_refresh_updates_open_controls_and_empty_is_not_legacy() {
    let mut app = app(None);
    assert_eq!(app.thinking_levels().len(), 6);
    app.open_thinking_picker();
    let mut choices = app.choices.clone();
    choices[0]
        .capabilities
        .as_mut()
        .expect("caps")
        .thinking_levels = Some(Vec::new());
    app.apply(HarnessEvent::ModelsRefreshed {
        choices,
        warning: None,
    });
    assert!(app.thinking_levels().is_empty());
    assert!(app.thinking_step(true, true).is_none());
    assert!(
        app.overlay
            .as_ref()
            .and_then(|overlay| overlay.list())
            .expect("picker")
            .choices
            .iter()
            .all(|choice| !choice.ready)
    );
}

#[test]
fn authoritative_events_update_thinking_and_old_events_keep_the_current_setting() {
    let mut app = app(Some(vec!["low", "xhigh"]));
    app.apply(HarnessEvent::ModelChanged {
        cursor: Cursor::ZERO,
        model: app.model.clone(),
        thinking: Some("xhigh".into()),
    });
    assert_eq!(app.thinking, "xhigh");
    let old: HarnessEvent = serde_json::from_value(serde_json::json!({"event":"model_changed","cursor":0,"model":{"name":"p/m","context_window":1000}})).expect("legacy event");
    app.apply(old);
    assert_eq!(app.thinking, "xhigh");
    let mut choices = app.choices.clone();
    choices[0].reasoning = false;
    choices[0].capabilities = None;
    app.apply(HarnessEvent::ModelsRefreshed {
        choices,
        warning: None,
    });
    assert!(!app.model_reasons);
    assert_eq!(app.thinking_levels(), [ThinkingLevel::Off]);
}
