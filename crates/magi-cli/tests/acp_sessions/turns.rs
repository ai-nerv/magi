use super::*;
use magi_testkit::Mind;

pub(super) fn synthetic(fixture: &Fixture, mind: &Mind) {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let native = std::fs::read_link(fixture.root.join("bin/melchior")).expect("selected melchior");
    std::fs::remove_file(fixture.root.join("bin/melchior")).expect("replace fixture link");
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in models|ask) exec '{}' \"$@\" ;; *) exec '{}' \"$@\" ;; esac\n",
        mind.program().display(),
        native.display()
    );
    let path = fixture.root.join("bin/melchior");
    std::fs::write(&path, script).expect("fixture model router");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("executable router");
    magi_model::scratch::executable_ready(&path);
    let search = std::env::var_os("PATH").expect("fixture tools path");
    for command in [
        "dirname", "cat", "mkdir", "rm", "rmdir", "touch", "sleep", "bwrap", "head",
    ] {
        let executable = std::env::split_paths(&search)
            .map(|directory| directory.join(command))
            .find(|path| path.is_file())
            .expect("shell fixture tool");
        symlink(executable, fixture.root.join("bin").join(command)).expect("selected shell tool");
    }
    std::fs::write(
        fixture.root.join("c/magi/init.lua"),
        "magi.model = 'fake/one'\nmagi.helpers = { memory = false }\nmagi.may_spawn = false\n",
    )
    .expect("synthetic-only model");
}

pub(super) async fn session(fixture: &mut Fixture) -> Value {
    let result = fixture
        .call(
            2,
            "session/new",
            json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
        )
        .await;
    assert!(result.get("result").is_some(), "{result}");
    result["result"]["sessionId"].clone()
}

pub(super) async fn prompt(fixture: &mut Fixture, id: u64, session: &Value, text: &str) {
    fixture
        .send(
            json!({"jsonrpc":"2.0", "id":id, "method":"session/prompt", "params":{
                "sessionId":session, "prompt":[{"type":"text", "text":text}]
            }}),
        )
        .await;
}

#[tokio::test]
async fn native_acp_prompt_uses_native_turns_and_preserves_resource_references() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::answering("acp-native-prompt", "native answer");
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    let uri = format!("file://{}", fixture.root.join("not-read.txt").display());
    std::fs::write(
        fixture.root.join("not-read.txt"),
        "not-implicitly-read-marker",
    )
    .expect("resource");
    fixture
        .send(
            json!({"jsonrpc":"2.0", "id":3, "method":"session/prompt", "params":{
                "sessionId":session, "prompt":[{"type":"text", "text":"literal prompt"},
                    {"type":"resource_link", "uri":uri, "name":"reference"}]
            }}),
        )
        .await;
    let mut chunks = String::new();
    let mut admitted = false;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 3);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            break;
        }
        assert_eq!(frame["params"]["sessionId"], session);
        match frame["params"]["update"]["sessionUpdate"].as_str() {
            Some("user_message_chunk") => admitted = true,
            Some("agent_message_chunk") => chunks.push_str(
                frame["params"]["update"]["content"]["text"]
                    .as_str()
                    .expect("text"),
            ),
            _ => {}
        }
    }
    assert!(admitted);
    assert_eq!(chunks, "native answer");
    assert_eq!(mind.asked(), 1);
    assert!(mind.heard().contains("literal prompt"));
    assert!(mind.heard().contains(&uri));
    assert!(!mind.heard().contains("not-implicitly-read-marker"));
    prompt(&mut fixture, 3, &session, "reuse completed request id").await;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 3);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            break;
        }
    }
    assert_eq!(mind.asked(), 2);
    assert!(
        mind.asks()[1].contains("literal prompt"),
        "native history retained"
    );
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_prompt_cancels_without_admitting_a_concurrent_prompt() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::controlled("acp-native-cancel");
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "first").await;
    loop {
        let frame = fixture.read().await;
        assert!(frame.get("id").is_none(), "no early completion: {frame}");
        if frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
            break;
        }
    }
    prompt(&mut fixture, 4, &session, "not admitted").await;
    let refusal = fixture.read().await;
    assert_eq!(refusal["id"], 4);
    assert!(refusal.get("error").is_some(), "{refusal}");
    fixture
        .send(json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":session}}))
        .await;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 3);
            assert_eq!(frame["result"]["stopReason"], "cancelled", "{frame}");
            break;
        }
    }
    assert_eq!(mind.asked(), 1);
    assert!(!mind.heard().contains("not admitted"));
    mind.release(1);
    prompt(&mut fixture, 5, &session, "after cancel").await;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 5);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            break;
        }
    }
    assert_eq!(mind.asked(), 2);
    assert!(!mind.overlapped());
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_cancellation_remains_live_while_another_session_starts() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::controlled("acp-starting-cancel");
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "working").await;
    loop {
        let frame = fixture.read().await;
        assert!(frame.get("id").is_none(), "{frame}");
        if frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
            break;
        }
    }
    let config = fixture.root.join("c/magi/init.lua");
    std::fs::remove_file(&config).expect("replace fixture config");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &config,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .expect("stall next startup");
    fixture
        .send(
            json!({"jsonrpc":"2.0","id":4,"method":"session/new","params":{
                "cwd":fixture.root.join("b"), "mcpServers":[]
            }}),
        )
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while fixture.host_pids().len() != 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "second host starting"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    fixture
        .send(json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":session}}))
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = fixture.read().await;
            if frame.get("id").is_some() {
                assert_eq!(frame["id"], 3, "startup stays pending");
                assert_eq!(frame["result"]["stopReason"], "cancelled", "{frame}");
                break;
            }
        }
    })
    .await
    .expect("cancellation does not wait for another startup");
    fixture.close().await;
}

pub(super) fn tooling(name: &str, tool: &str, arguments: Value) -> Mind {
    let call = magi_testkit::mind::call_lines("c1", tool, &arguments.to_string());
    let first: Vec<_> = call.iter().map(String::as_str).collect();
    Mind::turns(
        name,
        &[
            &first,
            &[
                &magi_testkit::mind::text_line("after tools"),
                &magi_testkit::mind::stop_line(),
            ],
        ],
    )
}

#[tokio::test]
async fn native_acp_tool_round_waits_for_native_result_and_followup() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let path = fixture.root.join("a/note.txt");
    std::fs::write(&path, "native-tool-result-marker").expect("resource");
    let mind = tooling("acp-tool-round", "read", json!({"path":path}));
    synthetic(&fixture, &mind);
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(fixture.root.join("c/magi/init.lua"))
            .expect("config"),
        "magi.allow = {{ {{ verb = 'read', directory = '{}' }} }}",
        fixture.root.join("a").display()
    )
    .expect("fixture-only read grant");
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "read the note").await;
    let mut called = false;
    let mut result_seen = false;
    let mut answer = String::new();
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 3);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            break;
        }
        let _: agent_client_protocol_schema::v1::SessionNotification =
            serde_json::from_value(frame["params"].clone()).expect("official update schema");
        let update = &frame["params"]["update"];
        match update["sessionUpdate"].as_str() {
            Some("tool_call") => called = true,
            Some("tool_call_update") => {
                assert_eq!(update["status"], "completed", "{update}");
                assert!(
                    update.to_string().contains("native-tool-result-marker"),
                    "{update}"
                );
                result_seen = true;
            }
            Some("agent_message_chunk") => {
                answer.push_str(update["content"]["text"].as_str().expect("text"))
            }
            _ => {}
        }
    }
    assert!(called && result_seen);
    assert_eq!(answer, "after tools");
    assert_eq!(mind.asked(), 2);
    assert!(mind.asks()[1].contains("native-tool-result-marker"));
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_disconnect_during_generation_reaps_owned_provider_work() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::controlled("acp-prompt-disconnect");
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "disconnect").await;
    loop {
        let frame = fixture.read().await;
        assert!(frame.get("id").is_none(), "{frame}");
        if frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
            break;
        }
    }
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_parent_death_during_generation_stops_owned_provider_work() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::controlled("acp-parent-death-active");
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "parent death while generating").await;
    loop {
        let frame = fixture.read().await;
        assert!(frame.get("id").is_none(), "{frame}");
        if frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
            break;
        }
    }
    assert_eq!(mind.asked(), 1);
    let provider: u32 = std::fs::read_to_string(
        mind.program()
            .parent()
            .expect("provider root")
            .join("active/pid"),
    )
    .expect("active provider pid")
    .trim()
    .parse()
    .expect("provider pid");
    assert!(Path::new(&format!("/proc/{provider}")).exists());
    fixture
        .child
        .kill()
        .await
        .expect("kill adapter without graceful shutdown");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining: Vec<_> = std::fs::read_dir("/proc")
            .expect("proc")
            .flatten()
            .filter(|entry| {
                std::fs::read_link(entry.path().join("cwd"))
                    .is_ok_and(|cwd| cwd.starts_with(&*fixture.root))
            })
            .map(|entry| entry.file_name())
            .collect();
        if remaining.is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "adapter death left owned processes {remaining:?}; provider {provider}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !Path::new(&format!("/proc/{provider}")).exists(),
        "provider reaped"
    );
}
