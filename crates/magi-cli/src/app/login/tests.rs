use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn account(id: &str, method: &str, saved: bool) -> Account {
    Account {
        id: id.into(),
        name: if id == "mistral" {
            "Mistral".into()
        } else {
            id.into()
        },
        method: method.into(),
        status: if saved {
            "Signed in".into()
        } else {
            "Not signed in".into()
        },
        source: if saved { "saved".into() } else { "none".into() },
        saved,
        available: true,
        variables: if method == "api-key" {
            vec!["MISTRAL_API_KEY".into()]
        } else {
            Vec::new()
        },
    }
}

fn listed() -> App {
    let mut app = App::new();
    let Some(UiCommand::Authenticate {
        request,
        action: Request::List,
    }) = app.open_logins()
    else {
        panic!("list");
    };
    app.apply(magi_proto::HarnessEvent::Authentication {
        request,
        reply: Reply::Finished {
            providers: vec![
                account("mistral", "api-key", false),
                account("OAuth fixture", "oauth", true),
                account("device-fixture", "oauth-device", false),
            ],
            error: None,
        },
    });
    app
}

#[test]
fn device_sign_in_shows_code_without_entering_editor_and_clears_on_finish_or_cancel() {
    for cancel in [false, true] {
        let mut app = listed();
        app.editor.insert_str("untouched draft");
        app.choose_login("provider:device-fixture");
        let Some(UiCommand::Authenticate {
            request,
            action: Request::Login { provider },
        }) = app.choose_login("add")
        else {
            panic!("device login");
        };
        assert_eq!(provider, "device-fixture");
        app.login_answer(
            request,
            Reply::Device {
                url: "https://provider.invalid/device".into(),
                code: "ABCD-EFGH".into(),
                expires_in: 900,
            },
        );
        assert!(text(&shown(&mut app, 80, 24)).contains("ABCD-EFGH"));
        assert_eq!(app.editor.text(), "untouched draft");
        if cancel {
            assert!(
                matches!(app.close_login(), Some(UiCommand::CancelAuthentication { request: id }) if id == request)
            );
            app.login_answer(
                request,
                Reply::Device {
                    url: "https://provider.invalid/device".into(),
                    code: "STALE-CODE".into(),
                    expires_in: 900,
                },
            );
            assert!(app.pane.is_none());
        } else {
            app.login_answer(
                request,
                Reply::Finished {
                    providers: vec![],
                    error: Some("Denied".into()),
                },
            );
            assert!(!text(&shown(&mut app, 80, 24)).contains("ABCD-EFGH"));
        }
        assert!(app.login.device_code.is_none());
        assert!(app.login.expires_in.is_none());
    }
}

fn shown(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            if let Some(pane) = &app.pane {
                let area = magi_tui::pane::Pane::area(frame.area());
                if area.width > 3 && area.height > 4 {
                    frame.render_widget(ratatui::widgets::Paragraph::new(pane.rows.clone()), area);
                }
            }
        })
        .expect("render");
    terminal.backend().buffer().clone()
}

fn text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|c| c.symbol()).collect()
}

#[test]
fn login_key_and_paste_never_enter_prompt_history_or_rendered_cells() {
    let mut app = listed();
    app.editor.insert_str("my existing draft");
    app.choose_login("provider:mistral");
    app.choose_login("add");
    assert!(app.login_paste("synthetic-private-key\n"));
    assert_eq!(app.editor.text(), "my existing draft");
    for (width, height) in [(80, 24), (160, 28), (20, 8), (1, 1), (0, 0)] {
        let view = text(&shown(&mut app, width, height));
        assert!(!view.contains("synthetic-private-key"));
    }
    let command = app
        .login_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .flatten()
        .expect("save");
    assert!(!format!("{command:?}").contains("synthetic-private-key"));
    assert!(
        matches!(command, UiCommand::Authenticate { action: Request::SetKey { key, .. }, .. } if key == "synthetic-private-key")
    );
    assert!(text(&shown(&mut app, 80, 24)).contains("Contacting Melchior"));
    assert!(matches!(app.login.page, Page::Waiting(_)));
    assert_eq!(app.editor.text(), "my existing draft");
}

#[test]
fn removal_requires_explicit_confirmation_and_escape_discards_secret() {
    let mut app = listed();
    app.choose_login("provider:OAuth fixture");
    assert!(app.choose_login("remove").is_none());
    assert_eq!(app.pane.as_ref().expect("pane").chosen(), Some("keep"));
    assert!(matches!(
        app.choose_login("confirm-remove"),
        Some(UiCommand::Authenticate {
            action: Request::Remove { .. },
            ..
        })
    ));
    let cancelled = app.close_login();
    assert!(matches!(
        cancelled,
        Some(UiCommand::CancelAuthentication { .. })
    ));
    app = listed();
    app.choose_login("provider:mistral");
    app.choose_login("add");
    app.login_paste("private-secret");
    app.login_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(app.login.page, Page::List));
    assert!(app.pane.is_none());
}

#[test]
fn stale_oauth_replies_cannot_reopen_closed_float_or_populate_trace() {
    let mut app = listed();
    app.choose_login("provider:OAuth fixture");
    let Some(UiCommand::Authenticate { request, .. }) = app.choose_login("add") else {
        panic!("oauth");
    };
    app.close_login();
    app.apply(magi_proto::HarnessEvent::Authentication {
        request,
        reply: Reply::Browser {
            url: "https://fixture.invalid/?state=private-state".into(),
        },
    });
    assert!(app.pane.is_none());
    assert!(matches!(app.login.page, Page::List));
}

#[test]
fn paste_outside_secret_field_is_swallowed_and_bad_input_is_not_saved() {
    let mut app = listed();
    assert!(app.login_paste("accidental-secret"));
    app.choose_login("provider:mistral");
    app.choose_login("add");
    app.login_paste("has a space");
    assert!(matches!(&app.login.page, Page::Key(key) if key.is_empty()));
    app.login_paste(&"x".repeat(8193));
    assert!(matches!(&app.login.page, Page::Key(key) if key.is_empty()));
    app.login_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(matches!(&app.login.page, Page::Key(key) if key == "q"));
}

#[test]
fn view_only_screen_cannot_open_authentication_or_modify_saved_logins() {
    let mut app = listed();
    app.view_only = true;
    assert!(app.open_logins().is_none());
    assert!(app.choose_login("add").is_none());
}

#[test]
fn secret_input_keeps_ownership_if_a_pane_is_replaced_and_agent_switch_drops_it() {
    let mut app = listed();
    app.choose_login("provider:mistral");
    app.choose_login("add");
    app.pane = None;
    assert!(app.login_paste("private-sentinel"));
    assert!(app.pane_titled("login"));
    assert!(app.editor.text().is_empty());
    app.attach_to(None);
    assert!(matches!(app.login.page, Page::List));
    assert!(app.login.pending.is_none());
}

#[test]
fn login_float_renders_at_normal_and_wide_terminal_sizes() {
    for (width, height) in [(80, 24), (160, 28)] {
        let mut app = listed();
        let buffer = shown(&mut app, width, height);
        let cells = text(&buffer);
        assert!(cells.contains("Mistral"));
        assert!(cells.contains("OAuth fixture"));
        app.choose_login("provider:mistral");
        app.choose_login("add");
        app.login_paste("private-preview-key");
        let buffer = shown(&mut app, width, height);
        assert!(!text(&buffer).contains("private-preview-key"));
        assert!(text(&buffer).contains("●●●●"));
    }
}
