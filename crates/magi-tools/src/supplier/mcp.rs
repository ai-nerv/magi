use magi_proto::permit::Action;
use magi_proto::tooling::Card;
use serde_json::Value;

pub(super) fn with_catalog(configured: &str, cards: &[Card]) -> String {
    let Ok(Value::Object(mut settings)) = serde_json::from_str(if configured.is_empty() {
        "{}"
    } else {
        configured
    }) else {
        return configured.into();
    };
    if settings.contains_key("mcp_servers")
        || cards.iter().any(|card| card.name.starts_with("mcp_"))
    {
        let catalog: Vec<_> = cards
            .iter()
            .filter(|card| card.name.starts_with("mcp_"))
            .collect();
        settings.insert("mcp_catalog".into(), serde_json::json!(catalog));
        return Value::Object(settings).to_string();
    }
    configured.into()
}

pub(super) fn uses_library(remote: bool, door: Option<&str>) -> bool {
    match door {
        Some(value) => value.trim() == "library",
        None => remote,
    }
}

/// What an MCP tool is asked about: its own name, never a path under the session's directory and
/// never anything the model's arguments say.
pub(super) fn permission(card: &Card) -> Option<Action> {
    if !card.name.starts_with("mcp_") {
        return None;
    }
    Some(match card.needs.as_deref()? {
        "reach" => Action::Network {
            host: card.name.clone(),
        },
        _ => Action::Run {
            command: card.name.clone(),
            program: card.name.clone(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_catalog_preserves_tools_without_magi_server_definitions() {
        let cards: Vec<Card> = serde_json::from_value(serde_json::json!([
            {"name":"mcp__generic__lookup","description":"lookup","parameters":{},"group":"mcp:generic"},
            {"name":"mcp__generic__long__lookup","description":"another lookup","parameters":{},"group":"mcp:generic__long"},
            {"name":"mcp_generic_resources","description":"resources","parameters":{}},
            {"name":"tools","description":"manual","parameters":{}}
        ])).expect("wire cards");
        let configured = with_catalog(
            r#"{"mcp_servers":{"generic":{"command":"private-launcher"},"generic__long":{"command":"other-launcher"}}}"#,
            &cards,
        );
        let configured: Value = serde_json::from_str(&configured).expect("settings");
        let catalog = configured["mcp_catalog"].as_array().expect("catalog");
        assert_eq!(catalog.len(), 3);
        assert_eq!(catalog[0]["name"], "mcp__generic__lookup");
        assert_eq!(catalog[1]["name"], "mcp__generic__long__lookup");
        assert_eq!(catalog[2]["name"], "mcp_generic_resources");
        assert!(
            !configured["mcp_catalog"]
                .to_string()
                .contains("private-launcher")
        );
        for settings in ["", "{}"] {
            let configured: Value =
                serde_json::from_str(&with_catalog(settings, &cards)).expect("directory catalog");
            assert_eq!(
                configured["mcp_catalog"].as_array().expect("catalog").len(),
                3
            );
            assert!(configured.get("mcp_servers").is_none());
        }
    }

    #[test]
    fn lost_mcp_responses_are_not_replayed() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        use std::time::Duration;
        const CHILD: &str = "MAGI_MCP_REPLY_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let dir = magi_model::scratch::Scratch::new("magi-mcp", "no-replay");
            let result = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "supplier::mcp::tests::lost_mcp_responses_are_not_replayed",
                ])
                .env(CHILD, "1")
                .env(super::super::DOOR, "library")
                .current_dir(&*dir)
                .output()
                .expect("isolated child");
            assert!(
                result.status.success(),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let root = std::env::current_dir().expect("root");
        let program = root.join("fake-tool");
        let marker = root.join("replayed");
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf replayed > replayed\nprintf '{}\\n'\n",
        )
        .expect("fake tool");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
        let program = program.to_str().expect("program path");
        // A daemon that lives as long as this process: its stdin closes when the process ends.
        let alive = || {
            std::process::Command::new("sh")
                .args(["-c", "read line"])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .expect("daemon stand-in")
        };
        let call = magi_proto::tooling::Call {
            tool: "mcp__remote__change".into(),
            args: serde_json::json!({}),
            cwd: String::new(),
            answered: None,
        };
        let context = crate::holding::Context::default();

        // The call reached the daemon and its reply was lost: it is never made again.
        let socket = root.join("reply.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        super::super::door::adopt(program, &context, socket, alive()).expect("adopted");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            let mut head = [0_u8; 4];
            stream.read_exact(&mut head).expect("request length");
            let mut request = vec![0_u8; u32::from_be_bytes(head) as usize];
            stream.read_exact(&mut request).expect("request frame");
            assert!(String::from_utf8_lossy(&request).contains("mcp__remote__change"));
            let mut lost = 8_u32.to_be_bytes().to_vec();
            lost.extend_from_slice(b"not JSON");
            stream.write_all(&lost).expect("lost result");
        });
        let error = super::super::run_jailed(program, &call, "", None, &crate::Uncancelled)
            .expect_err("unreadable response");
        server.join().expect("server");
        assert!(error.contains("not retried"), "{error}");
        assert!(
            !marker.exists(),
            "remote call was replayed through the command door"
        );

        // Nothing reached a daemon, so nothing can be replayed: the call goes by a spawn instead.
        super::super::door::adopt(program, &context, root.join("nobody.sock"), alive())
            .expect("adopted");
        let error = super::super::run_jailed(program, &call, "", None, &crate::Uncancelled)
            .expect_err("the fake tool answers no family reply");
        assert!(!error.contains("not retried"), "{error}");
        assert!(marker.exists(), "a call that never left was not run at all");
    }

    #[test]
    fn mcp_sessions_use_the_persistent_door_unless_explicitly_overridden() {
        assert!(uses_library(true, None));
        assert!(!uses_library(false, None));
        assert!(uses_library(true, Some("library")));
        assert!(!uses_library(true, Some("command")));
    }

    #[test]
    fn mcp_permission_subjects_do_not_come_from_model_arguments() {
        let mut card = Card {
            name: "mcp__remote__execute".into(),
            description: String::new(),
            parameters: serde_json::json!({}),
            needs: Some("run".into()),
            deferred: false,
        };
        let args = serde_json::json!({"command":"git status","host":"allowed.example","path":"x"});
        let wanted = |card: &Card| super::super::wants(card, &args);
        assert!(
            matches!(wanted(&card), Some(Action::Run { command, program }) if program == card.name && command == card.name)
        );
        card.needs = Some("reach".into());
        assert!(matches!(wanted(&card), Some(Action::Network { host }) if host == card.name));
        for needs in ["read", "write"] {
            card.needs = Some(needs.into());
            assert!(
                matches!(wanted(&card), Some(Action::Run { program, .. }) if program == card.name),
                "{needs}"
            );
        }
    }
}
