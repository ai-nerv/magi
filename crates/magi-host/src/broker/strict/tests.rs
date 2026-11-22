use super::*;
use magi_model::{Message, Tool, scratch::Scratch};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

async fn ready(role: &std::path::Path) -> std::io::Result<()> {
    use std::process::Stdio;
    use std::time::Duration;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match tokio::process::Command::new(role)
                .arg("--probe")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status()
                .await
            {
                Ok(status) if status.success() => return Ok(()),
                Ok(_) => return Err(std::io::Error::other("role probe failed")),
                Err(error) if error.raw_os_error() == Some(26) => {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "role remained busy"))?
}

#[tokio::test]
async fn a_busy_fixture_is_probed_before_the_contract_and_other_errors_are_not_retried() {
    let dir = Scratch::new("magi-strict", "busy");
    let role = dir.join("role");
    std::fs::write(&role, "#!/bin/sh\nexit 0\n").expect("role");
    std::fs::set_permissions(&role, std::fs::Permissions::from_mode(0o700)).expect("mode");
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&role)
        .expect("writer");
    let error = tokio::process::Command::new(&role)
        .status()
        .await
        .expect_err("busy");
    assert_eq!(error.raw_os_error(), Some(26));
    let release = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        drop(writer);
    });
    ready(&role).await.expect("closed writer allows execution");
    release.await.expect("released");
    let missing = ready(&dir.join("missing")).await.expect_err("missing role");
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
}

#[tokio::test]
async fn required_policy_is_negotiated_before_asking_a_replacement_model_role() {
    for advertised in [None, Some(false), Some(true)] {
        let dir = Scratch::new("magi-strict", "role");
        let role = dir.join("model-role");
        let capture = dir.join("ask.json");
        let mut card = json!({"id":"p/m","provider":"p","name":"M","api":"fixture"});
        if let Some(supported) = advertised {
            card["capabilities"] = json!({"strict_tools":supported});
        }
        let reply = json!({"ok":true,"result":[card]});
        let stop = magi_testkit::mind::stop_line();
        std::fs::write(&role, format!("#!/bin/sh\ncase \"$1\" in\n--probe) exit 0 ;;\nmodels) printf '%s\\n' '{reply}' ;;\nask) cat > \"$CAPTURE\"; printf '%s\\n' '{stop}' ;;\nesac\n")).expect("role");
        std::fs::set_permissions(&role, std::fs::Permissions::from_mode(0o700))
            .expect("executable");
        ready(&role).await.expect("fixture is executable");
        let environ = BTreeMap::from([("CAPTURE".into(), capture.to_string_lossy().into_owned())]);
        let mut context = Context {
            tools: vec![Tool {
                name: "lookup".into(),
                description: "Lookup".into(),
                parameters: json!({"type":"object"}),
                strict: Some(ToolStrictness::Require),
            }],
            ..Default::default()
        };
        let result = super::super::ask_using(
            (role.to_str().expect("path"), &environ),
            "p/m",
            &context,
            &Default::default(),
            |_| {},
            |_| {},
        )
        .await;
        if advertised == Some(true) {
            result.expect("supported role");
            let request: Value =
                serde_json::from_slice(&std::fs::read(&capture).expect("ask")).expect("request");
            assert_eq!(request["context"]["tools"][0]["strict"], "require");
            std::fs::remove_file(&capture).expect("reset capture");
        } else {
            assert_eq!(result.expect_err("unsupported role").why, Refusal::Invalid);
            assert!(!capture.exists());
        }
        context.tools[0].strict = Some(ToolStrictness::Prefer);
        super::super::ask_using(
            (role.to_str().expect("path"), &environ),
            "p/m",
            &context,
            &Default::default(),
            |_| {},
            |_| {},
        )
        .await
        .expect("preferred is optional");
        assert!(capture.exists());
    }
}

#[tokio::test]
async fn inline_required_tools_negotiate_the_current_state_before_a_replacement_role_is_asked() {
    for advertised in [None, Some(false), Some(true)] {
        for change in ["add", "remove", "replace", "readd"] {
            let dir = Scratch::new("magi-strict", "tool-history");
            let role = dir.join("model-role");
            let capture = dir.join("ask.json");
            let mut card = json!({"id":"p/m","provider":"p","name":"M","api":"fixture"});
            if let Some(supported) = advertised {
                card["capabilities"] = json!({"strict_tools":supported});
            }
            let reply = json!({"ok":true,"result":[card]});
            let stop = magi_testkit::mind::stop_line();
            std::fs::write(&role, format!("#!/bin/sh\ncase \"$1\" in\n--probe) exit 0 ;;\nmodels) printf '%s\\n' '{reply}' ;;\nask) cat > \"$CAPTURE\"; printf '%s\\n' '{stop}' ;;\nesac\n")).expect("role");
            std::fs::set_permissions(&role, std::fs::Permissions::from_mode(0o700))
                .expect("executable");
            ready(&role).await.expect("fixture is executable");
            let environ =
                BTreeMap::from([("CAPTURE".into(), capture.to_string_lossy().into_owned())]);
            let tool = Tool {
                name: "lookup".into(),
                description: "Lookup".into(),
                parameters: json!({"type":"object"}),
                strict: Some(ToolStrictness::Require),
            };
            let mut context = Context {
                messages: vec![
                    Message::user("first"),
                    Message::tool_changes(vec![tool.clone()], vec![]),
                ],
                ..Context::default()
            };
            if change == "remove" || change == "readd" {
                context
                    .messages
                    .push(Message::tool_changes(vec![], vec![tool.name.clone()]));
            }
            if change == "replace" {
                let mut preferred = tool.clone();
                preferred.strict = Some(ToolStrictness::Prefer);
                context
                    .messages
                    .push(Message::tool_changes(vec![preferred], vec![]));
            }
            if change == "readd" {
                context
                    .messages
                    .push(Message::tool_changes(vec![tool], vec![]));
            }
            let saved = context.clone();
            let result = super::super::ask_using(
                (role.to_str().expect("path"), &environ),
                "p/m",
                &context,
                &Default::default(),
                |_| {},
                |_| {},
            )
            .await;
            if advertised == Some(true) || matches!(change, "remove" | "replace") {
                result.expect("current policy is supported");
                let request: Value =
                    serde_json::from_slice(&std::fs::read(&capture).expect("capture"))
                        .expect("JSON");
                assert_eq!(
                    request["context"],
                    serde_json::to_value(&context).expect("context")
                );
            } else {
                assert_eq!(
                    result.expect_err("unsupported required tool").why,
                    Refusal::Invalid
                );
                assert!(!capture.exists());
            }
            assert_eq!(context, saved);
        }
    }
}

#[test]
fn tool_policy_replay_is_role_checked_and_bounded_before_model_listing() {
    let mut context = Context {
        messages: vec![Message::tool_changes(vec![], vec![])],
        ..Context::default()
    };
    for role in [Role::User, Role::Assistant, Role::Tool] {
        context.messages[0].role = role;
        assert!(required(&context).is_err());
    }
    context.messages[0].role = Role::System;
    context.messages[0].content.push(Content::ToolChanges {
        added: vec![],
        removed: vec![],
    });
    assert!(required(&context).is_err());
    context.messages[0].content.pop();
    context.tools = (0..4096)
        .map(|i| Tool {
            name: format!("t{i}"),
            description: String::new(),
            parameters: json!({}),
            strict: None,
        })
        .collect();
    assert_eq!(required(&context), Ok(false));
    context.messages.push(Message::tool_changes(
        vec![Tool {
            name: "one-too-many".into(),
            description: String::new(),
            parameters: json!({}),
            strict: None,
        }],
        vec![],
    ));
    assert!(required(&context).is_err());
    context.tools.clear();
    context.messages = vec![Message::tool_changes(vec![], vec!["missing".into(); 16384])];
    assert_eq!(required(&context), Ok(false));
    context
        .messages
        .push(Message::tool_changes(vec![], vec!["missing".into()]));
    assert!(required(&context).is_err());
}
