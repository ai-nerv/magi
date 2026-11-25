use super::*;
use magi_proto::{
    HarnessEvent, UiCommand,
    authentication::{Account, Reply},
};
use ratatui::{Terminal, backend::TestBackend};

#[cfg(test)]
mod preview;

#[test]
fn actual_login_floats_render_without_credentials_at_normal_and_wide_sizes() {
    for (width, height) in [(80, 24), (160, 28)] {
        let mut app = App::new();
        let Some(UiCommand::Authenticate { request, .. }) = app.open_logins() else {
            panic!("list");
        };
        let providers = [
            ("mistral", "Mistral", "api-key", false),
            ("oauth-fixture", "OAuth fixture", "oauth", true),
            ("device-fixture", "Device fixture", "oauth-device", false),
        ]
        .into_iter()
        .map(|(id, name, method, saved)| Account {
            id: id.into(),
            name: name.into(),
            method: method.into(),
            saved,
            available: true,
            status: if saved { "Signed in" } else { "Not signed in" }.into(),
            source: if saved { "saved" } else { "none" }.into(),
            variables: Vec::new(),
        })
        .collect();
        app.apply(HarnessEvent::Authentication {
            request,
            reply: Reply::Finished {
                providers,
                error: None,
            },
        });
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        for (state, choices) in [
            ("providers", vec![]),
            ("key", vec!["provider:mistral", "add"]),
            ("remove", vec!["back", "provider:oauth-fixture", "remove"]),
            ("device", vec!["back", "provider:device-fixture"]),
        ] {
            for choice in choices {
                app.choose_login(choice);
            }
            if state == "key" {
                app.login_paste("private-preview-key");
            }
            if state == "device" {
                let Some(UiCommand::Authenticate { request, .. }) = app.choose_login("add") else {
                    panic!("device sign-in");
                };
                app.apply(HarnessEvent::Authentication {
                    request,
                    reply: Reply::Device {
                        url: "https://provider.invalid/device".into(),
                        code: "ABCD-EFGH".into(),
                        expires_in: 900,
                    },
                });
            }
            terminal
                .draw(|frame| {
                    draw(frame, &mut app, &FooterData::default());
                })
                .expect("draw");
            let buffer = terminal.backend().buffer();
            let cells: String = buffer.content.iter().map(|c| c.symbol()).collect();
            assert!(!cells.contains("private-preview-key"));
            if state == "providers" {
                assert!(cells.contains("all") && cells.contains("Mistral"));
            }
            if state == "key" {
                assert!(cells.contains("API key") && cells.contains("●●●●"));
            }
            if state == "remove" {
                assert!(cells.contains("Remove saved login"));
            }
            if state == "device" {
                assert!(cells.contains("ABCD-EFGH"));
                assert!(cells.contains("copy code"));
            }
            preview::save(buffer, &format!("{state}-{width}x{height}"));
        }
    }
}
