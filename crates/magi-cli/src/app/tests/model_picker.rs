use super::super::*;
use magi_proto::{ModelChoice, ModelInfo, UiCommand, ask::Pricing};

fn choice(
    provider: &str,
    name: &str,
    ready: bool,
    pricing: Option<Pricing>,
    rank: Option<u64>,
    subscription: bool,
) -> ModelChoice {
    ModelChoice {
        vision: false,
        name: format!("{provider}/{name}"),
        provider: Some(provider.to_owned()),
        display_name: None,
        context_window: 200_000,
        requirement: if ready {
            String::new()
        } else {
            format!("set {}_API_KEY", provider.to_uppercase())
        },
        pricing,
        popularity_rank: rank,
        capabilities: None,
        wants_vars: Vec::new(),
        reasoning: true,
        subscription,
    }
}

fn rendered_text(app: &App, width: u16) -> String {
    let picker = app.overlay.as_ref().expect("picker").list().expect("list");
    let lines = magi_tui::picker::render(picker, width);
    assert_eq!(lines.len(), usize::from(picker.height()));
    lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn subscription_hides_unready_openrouter_models_and_price_columns_by_default() {
    let mut app = App::new();
    app.choices = vec![
        choice("qwen-token-plan", "qwen3-coder", true, None, None, false),
        choice(
            "openrouter",
            "anthropic/claude-sonnet-4.5",
            false,
            Some(Pricing {
                input: Some("3".into()),
                output: Some("15".into()),
            }),
            Some(2),
            false,
        ),
    ];

    app.open_model_picker();
    let shown = rendered_text(&app, 140);
    assert!(shown.contains("qwen-token-plan/qwen3-coder"), "{shown}");
    assert!(!shown.contains("openrouter/"), "{shown}");
    assert!(!shown.contains("In$/M"), "{shown}");
    assert!(!shown.contains("Out$/M"), "{shown}");
    assert!(!shown.contains("ORwk"), "{shown}");
    assert!(shown.contains("^a +1 locked"), "{shown}");

    app.toggle_model_locked();
    let expanded = rendered_text(&app, 140);
    assert!(
        expanded.contains("openrouter/anthropic/claude-sonnet-4.5"),
        "{expanded}"
    );
    assert!(
        expanded.contains("In$/M") && expanded.contains("ORwk"),
        "{expanded}"
    );
    assert!(expanded.contains("^a ready only"), "{expanded}");

    let picker = app
        .overlay
        .as_mut()
        .expect("picker")
        .picker()
        .expect("list");
    picker.next();
    assert_eq!(picker.take(), None);
    let refused = magi_tui::picker::render(picker, 140)[0].to_string();
    assert!(refused.contains("set OPENROUTER_API_KEY"), "{refused}");
    assert!(!refused.contains('\t'), "{refused}");
}

#[test]
fn multiple_ready_providers_open_on_active_provider_and_adapt_columns_per_tab() {
    let mut app = App::new();
    app.model = Some(ModelInfo {
        name: "qwen-token-plan/qwen3-coder".into(),
        context_window: 200_000,
    });
    app.choices = vec![
        choice(
            "openrouter",
            "anthropic/claude-sonnet-4.5",
            true,
            Some(Pricing {
                input: Some("3".into()),
                output: Some("15".into()),
            }),
            Some(1),
            false,
        ),
        choice("qwen-token-plan", "qwen3-coder", true, None, None, true),
    ];

    app.open_model_picker();
    let sub_view = rendered_text(&app, 140);
    assert!(
        sub_view.contains("qwen-token-plan/qwen3-coder"),
        "{sub_view}"
    );
    assert!(!sub_view.contains("openrouter/anthropic"), "{sub_view}");
    assert!(
        !sub_view.contains("In$/M") && !sub_view.contains("ORwk"),
        "{sub_view}"
    );
    assert!(sub_view.contains("←→ provider"), "{sub_view}");

    app.step_model_provider(true);
    let all_view = rendered_text(&app, 140);
    assert!(
        all_view.contains("qwen-token-plan/qwen3-coder"),
        "{all_view}"
    );
    assert!(
        all_view.contains("openrouter/anthropic/claude-sonnet-4.5"),
        "{all_view}"
    );
    assert!(
        all_view.contains("In$/M") && all_view.contains("ORwk"),
        "{all_view}"
    );

    app.step_model_provider(true);
    let or_view = rendered_text(&app, 140);
    assert!(
        or_view.contains("openrouter/anthropic/claude-sonnet-4.5"),
        "{or_view}"
    );
    assert!(
        !or_view.contains("qwen-token-plan/qwen3-coder"),
        "{or_view}"
    );
    assert!(
        or_view.contains("$3") && or_view.contains("#1"),
        "{or_view}"
    );
}

#[test]
fn login_account_models_action_opens_picker_scoped_to_that_provider() {
    let mut app = App::new();
    app.choices = vec![
        choice("mistral", "codestral", true, None, None, false),
        choice(
            "openrouter",
            "deepseek/v4",
            true,
            Some(Pricing {
                input: Some("0.3".into()),
                output: Some("1.2".into()),
            }),
            Some(5),
            false,
        ),
    ];
    let Some(UiCommand::Authenticate { request, .. }) = app.open_logins() else {
        panic!("open logins");
    };
    app.apply(HarnessEvent::Authentication {
        request,
        reply: magi_proto::authentication::Reply::Finished {
            providers: vec![magi_proto::authentication::Account {
                id: "mistral".into(),
                name: "Mistral".into(),
                method: "api-key".into(),
                status: "Saved API key".into(),
                source: "saved".into(),
                saved: true,
                available: true,
                variables: vec!["MISTRAL_API_KEY".into()],
            }],
            error: None,
        },
    });
    app.choose_login("provider:mistral");
    let command = app.choose_login("models");
    assert_eq!(command, Some(UiCommand::RefreshModels));
    assert!(app.pane.is_none());
    assert_eq!(app.picking, Some(Picking::Model));
    let shown = rendered_text(&app, 140);
    assert!(shown.contains("mistral/codestral"), "{shown}");
    assert!(!shown.contains("openrouter/deepseek"), "{shown}");
}

#[test]
fn subscription_model_card_omits_openrouter_pricing_and_routing_endpoints() {
    let mut app = App::new();
    app.model = Some(ModelInfo {
        name: "qwen-token-plan/qwen3-coder".into(),
        context_window: 200_000,
    });
    app.choices = vec![choice(
        "qwen-token-plan",
        "qwen3-coder",
        true,
        None,
        None,
        true,
    )];
    app.details = Some((
        "qwen-token-plan/qwen3-coder".into(),
        Ok(magi_tui::model_card::Details {
            description: Some("Coding model".into()),
            price: [0.3, 1.2, 0.0, 0.0],
            endpoints: vec![magi_tui::model_card::Endpoint {
                provider: "DeepInfra".into(),
                tag: Some("deepinfra".into()),
                price: [0.3, 1.2, 0.0, 0.0],
                ..Default::default()
            }],
            ..Default::default()
        }),
    ));
    app.show_model();
    let card = app
        .pane
        .as_ref()
        .expect("model pane")
        .rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(card.contains("Coding model"), "{card}");
    assert!(card.contains("subscription"), "{card}");
    assert!(card.contains("manage logins & plans"), "{card}");
    assert!(!card.contains("Price") && !card.contains("$0.30"), "{card}");
    assert!(!card.contains("DeepInfra"), "{card}");
    assert!(matches!(
        app.choose_on_model("login"),
        Some(UiCommand::Authenticate { .. })
    ));
}

#[test]
fn login_account_float_allows_selecting_a_ready_model_directly() {
    let mut app = App::new();
    app.choices = vec![choice(
        "qwen-token-plan",
        "qwen3-coder",
        true,
        None,
        None,
        false,
    )];
    let Some(UiCommand::Authenticate { request, .. }) = app.open_logins() else {
        panic!("open logins");
    };
    app.apply(HarnessEvent::Authentication {
        request,
        reply: magi_proto::authentication::Reply::Finished {
            providers: vec![magi_proto::authentication::Account {
                id: "qwen-token-plan".into(),
                name: "Qwen Token Plan".into(),
                method: "api-key".into(),
                status: "Saved API key".into(),
                source: "saved".into(),
                saved: true,
                available: true,
                variables: vec!["QWEN_TOKEN_PLAN_API_KEY".into()],
            }],
            error: None,
        },
    });
    app.choose_login("provider:qwen-token-plan");
    let command = app.choose_login("model:qwen-token-plan/qwen3-coder");
    assert_eq!(
        command,
        Some(UiCommand::SetModel {
            name: "qwen-token-plan/qwen3-coder".into()
        })
    );
    assert!(app.pane.is_none());
}
