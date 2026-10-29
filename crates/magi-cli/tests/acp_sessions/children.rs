use super::{turns::*, *};

async fn background_child(mode: &str) {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let marker = fixture.root.join("a/child-approved");
    let allowed = matches!(mode, "allow" | "background-cancel");
    let spawn = magi_testkit::mind::call_lines(
        "spawn-fixture",
        "spawn",
        &json!({"role":"reviewer","delegate":false,"prompt":"Perform the fixture child action"})
            .to_string(),
    );
    let action = magi_testkit::mind::call_lines(
        "child-action",
        "shell",
        &json!({"command":format!("touch {}", marker.display())}).to_string(),
    );
    let quoted = |lines: &[String]| {
        lines
            .iter()
            .map(|line| format!("'{}'", line.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let body = format!(
        r#"
if [ -n "${{MAGI_MELCHIOR_PARENT:-}}" ]; then
  if grep -q child-action "$here/ask.$n"; then
    printf '%s\n' '{{"event":"text","text":"child finished"}}' '{{"event":"stop","reason":"end_turn"}}'
  else
    sleep 2
    printf '%s\n' {action}
  fi
elif grep -q 'reported a finished turn' "$here/ask.$n"; then
  printf '%s\n' '{{"event":"text","text":"automatic child follow-up"}}'
  {waiting}
  printf '%s\n' '{{"event":"stop","reason":"end_turn"}}'
elif grep -q spawn-fixture "$here/ask.$n"; then
  printf '%s\n' '{{"event":"text","text":"parent finished"}}' '{{"event":"stop","reason":"end_turn"}}'
else
  printf '%s\n' {spawn}
fi
"#,
        action = quoted(&action),
        spawn = quoted(&spawn),
        waiting = if mode == "background-cancel" {
            "exec sleep 60"
        } else {
            ":"
        },
    );
    let mind = magi_testkit::Mind::scripted(&format!("acp-child-permission-{mode}"), &body);
    synthetic(&fixture, &mind);
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(fixture.root.join("c/magi/init.lua"))
            .expect("config"),
        "magi.may_spawn = true"
    )
    .expect("enable fixture child");
    let search = std::env::var_os("PATH").expect("fixture path");
    let grep = std::env::split_paths(&search)
        .map(|directory| directory.join("grep"))
        .find(|path| path.is_file())
        .expect("grep");
    std::os::unix::fs::symlink(grep, fixture.root.join("bin/grep")).expect("fixture grep");
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "Spawn the fixture child").await;
    let mut parent_finished = false;
    let mut spawn_approved = false;
    let mut spawn_started = false;
    let mut spawn_completed = false;
    loop {
        let frame = fixture.read().await;
        if frame["method"] == "session/request_permission" {
            assert_eq!(frame["params"]["sessionId"], session);
            let title = frame["params"]["toolCall"]["title"]
                .as_str()
                .expect("title");
            if title.starts_with("subagent:") {
                assert!(
                    parent_finished,
                    "fixture child asks after parent prompt completes"
                );
                assert!(!marker.exists());
                assert!(
                    spawn_started && spawn_completed,
                    "native spawn must run successfully"
                );
                assert!(
                    fixture.host_pids().len() >= 2,
                    "real child host remains alive"
                );
                if mode == "disconnect" {
                    let _retained = fixture.close().await;
                    assert!(!marker.exists());
                    return;
                }
                if mode == "cancel" {
                    fixture.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})).await;
                }
                fixture.send(json!({"jsonrpc":"2.0","id":frame["id"],"result":{
                    "outcome":{"outcome":"selected","optionId":if mode == "deny" { "deny" } else { "allow-0" }}
                }})).await;
                break;
            }
            assert!(title.starts_with("spawn:"), "{frame}");
            assert!(!spawn_approved);
            spawn_approved = true;
            fixture
                .send(json!({"jsonrpc":"2.0","id":frame["id"],"result":{
                    "outcome":{"outcome":"selected","optionId":"allow-0"}
                }}))
                .await;
        } else if frame.get("id").is_some() {
            assert_eq!(frame["id"], 3);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            parent_finished = true;
        } else if frame["params"]["update"]["toolCallId"] == "magi-tool-spawn-fixture" {
            let update = &frame["params"]["update"];
            match update["sessionUpdate"].as_str() {
                Some("tool_call") => {
                    assert_eq!(update["title"], "spawn");
                    spawn_started = true;
                }
                Some("tool_call_update") => {
                    assert_eq!(update["status"], "completed", "{frame}");
                    spawn_completed = true;
                }
                _ => {}
            }
        }
    }
    assert!(spawn_started && spawn_completed);
    if allowed {
        let mut wake_seen = false;
        loop {
            let frame = fixture.read().await;
            assert!(
                frame.get("id").is_none(),
                "no unsolicited or duplicate prompt response: {frame}"
            );
            assert_eq!(frame["params"]["sessionId"], session);
            let update = &frame["params"]["update"];
            if update["sessionUpdate"] == "user_message_chunk" {
                wake_seen |= update["content"]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("reported a finished turn"));
            }
            if update["sessionUpdate"] == "agent_message_chunk"
                && update["content"]["text"] == "automatic child follow-up"
            {
                assert!(
                    wake_seen,
                    "automatic native opening must precede its answer"
                );
                break;
            }
        }
        if mode == "background-cancel" {
            fixture.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})).await;
            loop {
                let frame = fixture.read().await;
                assert!(
                    frame.get("id").is_none(),
                    "background cancellation has no prompt response: {frame}"
                );
                assert_eq!(frame["params"]["sessionId"], session);
                if frame["params"]["update"]["content"]["text"] == "Automatic native turn cancelled"
                {
                    break;
                }
            }
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while if allowed {
        !marker.exists()
    } else {
        !mind.heard().contains("child-action") || mind.asked() < 4
    } {
        assert!(
            tokio::time::Instant::now() < deadline,
            "child action did not settle for {mode}; asks={}",
            serde_json::to_string(
                &mind
                    .asks()
                    .iter()
                    .filter_map(|ask| serde_json::from_str::<Value>(ask).ok())
                    .map(|ask| ask["context"]["messages"].clone())
                    .collect::<Vec<_>>()
            )
            .expect("fixture messages")
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _retained = fixture.close().await;
    assert_eq!(marker.exists(), allowed);
}

#[tokio::test]
async fn native_acp_background_child_permission_reaches_the_idle_parent_session() {
    background_child("allow").await;
}

#[tokio::test]
async fn native_acp_background_child_denial_does_not_run_the_action() {
    background_child("deny").await;
}

#[tokio::test]
async fn native_acp_background_child_cancel_retires_late_allow() {
    background_child("cancel").await;
}

#[tokio::test]
async fn native_acp_background_child_disconnect_reaps_without_action() {
    background_child("disconnect").await;
}

#[tokio::test]
async fn native_acp_background_automatic_turn_cancels_without_a_prompt_response() {
    background_child("background-cancel").await;
}
