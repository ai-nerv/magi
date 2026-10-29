use super::{turns::*, *};

async fn permission(fixture: &mut Fixture) -> Value {
    loop {
        let frame = fixture.read().await;
        if frame["method"] == "session/request_permission" {
            let _: agent_client_protocol_schema::v1::RequestPermissionRequest =
                serde_json::from_value(frame["params"].clone())
                    .expect("official permission schema");
            return frame;
        }
        assert!(
            frame.get("id").is_none(),
            "no completion before permission: {frame}"
        );
    }
}

async fn finish(fixture: &mut Fixture, expected: &str) {
    finish_id(fixture, 3, expected).await;
}

async fn finish_id(fixture: &mut Fixture, id: u64, expected: &str) {
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], id);
            assert_eq!(frame["result"]["stopReason"], expected, "{frame}");
            return;
        }
    }
}

async fn select(fixture: &mut Fixture, request: &Value, option: &str) {
    fixture
        .send(json!({"jsonrpc":"2.0", "id":request["id"], "result":{
            "outcome":{"outcome":"selected", "optionId":option}
        }}))
        .await;
}

#[tokio::test]
async fn native_acp_permission_allow_once_runs_the_selected_tool_and_preserves_results() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let path = fixture.root.join("a/note");
    std::fs::write(&path, "permission-read-marker").expect("resource");
    let mind = tooling("acp-permission-read", "read", json!({"path":path}));
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "read with permission").await;
    let request = permission(&mut fixture).await;
    assert_eq!(request["params"]["sessionId"], session);
    assert_ne!(request["params"]["toolCall"]["toolCallId"], "magi-tool-c1");
    assert_eq!(mind.asked(), 1);
    select(&mut fixture, &request, "allow-0").await;
    finish(&mut fixture, "end_turn").await;
    assert_eq!(mind.asked(), 2);
    assert!(mind.asks()[1].contains("permission-read-marker"));
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_permission_deny_never_runs_the_action() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let marker = fixture.root.join("a/must-not-exist");
    let mind = tooling(
        "acp-permission-deny",
        "shell",
        json!({"command":format!("touch {}", marker.display())}),
    );
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "deny permission").await;
    let request = permission(&mut fixture).await;
    assert!(!marker.exists());
    select(&mut fixture, &request, "deny").await;
    finish(&mut fixture, "end_turn").await;
    assert_eq!(mind.asked(), 2);
    assert!(!marker.exists());
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_cancelled_permission_late_allow_cannot_grant_or_affect_the_next_turn() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let marker = fixture.root.join("a/must-not-exist");
    let mind = tooling(
        "acp-permission-late",
        "shell",
        json!({"command":format!("touch {}", marker.display())}),
    );
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "cancel permission").await;
    let request = permission(&mut fixture).await;
    fixture
        .send(json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":session}}))
        .await;
    finish(&mut fixture, "cancelled").await;
    select(&mut fixture, &request, "allow-1").await;
    prompt(&mut fixture, 3, &session, "next turn").await;
    finish(&mut fixture, "end_turn").await;
    assert_eq!(mind.asked(), 2);
    assert!(!marker.exists());
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_permission_disconnect_cleans_up_without_running_the_action() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let marker = fixture.root.join("a/must-not-exist");
    let mind = tooling(
        "acp-permission-disconnect",
        "shell",
        json!({"command":format!("touch {}", marker.display())}),
    );
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "disconnect permission").await;
    permission(&mut fixture).await;
    let _retained = fixture.close().await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn native_acp_unoffered_permission_choice_closes_and_cleans_up_without_effects() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let marker = fixture.root.join("a/must-not-exist");
    let mind = tooling(
        "acp-permission-invalid",
        "shell",
        json!({"command":format!("touch {}", marker.display())}),
    );
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "invalid permission").await;
    let request = permission(&mut fixture).await;
    select(&mut fixture, &request, "allow-not-offered").await;
    let _retained = fixture.close_status(false).await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn native_acp_allow_once_requires_a_fresh_permission_for_the_next_action() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let path = fixture.root.join("a/note");
    std::fs::write(&path, "once-only-secret").expect("resource");
    let call = magi_testkit::mind::call_lines("c1", "read", &json!({"path":path}).to_string());
    let first: Vec<_> = call.iter().map(String::as_str).collect();
    let end = [
        magi_testkit::mind::text_line("after tools"),
        magi_testkit::mind::stop_line(),
    ];
    let followup: Vec<_> = end.iter().map(String::as_str).collect();
    let mind = magi_testkit::Mind::turns(
        "acp-permission-once",
        &[&first, &followup, &first, &followup],
    );
    synthetic(&fixture, &mind);
    let session = session(&mut fixture).await;
    prompt(&mut fixture, 3, &session, "first read").await;
    let first = permission(&mut fixture).await;
    select(&mut fixture, &first, "allow-0").await;
    finish(&mut fixture, "end_turn").await;
    prompt(&mut fixture, 3, &session, "second read").await;
    let second = permission(&mut fixture).await;
    assert_ne!(first["id"], second["id"]);
    assert_ne!(
        first["params"]["toolCall"]["toolCallId"],
        second["params"]["toolCall"]["toolCallId"]
    );
    select(&mut fixture, &second, "deny").await;
    finish(&mut fixture, "end_turn").await;
    assert_eq!(mind.asked(), 4);
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_permission_replies_are_correlated_when_sessions_answer_out_of_order() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let path_a = fixture.root.join("a/note");
    let path_b = fixture.root.join("b/note");
    std::fs::write(&path_a, "first-session-secret").expect("first resource");
    std::fs::write(&path_b, "second-session-secret").expect("second resource");
    let a = magi_testkit::mind::call_lines("c1", "read", &json!({"path":path_a}).to_string());
    let b = magi_testkit::mind::call_lines("c1", "read", &json!({"path":path_b}).to_string());
    let first: Vec<_> = a.iter().map(String::as_str).collect();
    let second: Vec<_> = b.iter().map(String::as_str).collect();
    let end = [
        magi_testkit::mind::text_line("after tools"),
        magi_testkit::mind::stop_line(),
    ];
    let followup: Vec<_> = end.iter().map(String::as_str).collect();
    let mind =
        magi_testkit::Mind::turns("acp-permission-correlated", &[&first, &second, &followup]);
    synthetic(&fixture, &mind);
    let a = session(&mut fixture).await;
    let b = fixture
        .call(
            2,
            "session/new",
            json!({"cwd":fixture.root.join("b"),"mcpServers":[]}),
        )
        .await;
    let b = b["result"]["sessionId"].clone();
    prompt(&mut fixture, 3, &a, "first pending read").await;
    let first = permission(&mut fixture).await;
    prompt(&mut fixture, 4, &b, "second pending read").await;
    let second = permission(&mut fixture).await;
    assert_ne!(first["id"], second["id"]);
    assert_eq!(first["params"]["sessionId"], a);
    assert_eq!(second["params"]["sessionId"], b);
    select(&mut fixture, &second, "allow-0").await;
    finish_id(&mut fixture, 4, "end_turn").await;
    assert_eq!(mind.asked(), 3);
    assert!(mind.asks()[2].contains("second-session-secret"));
    assert!(!mind.asks()[2].contains("first-session-secret"));
    select(&mut fixture, &first, "deny").await;
    finish(&mut fixture, "end_turn").await;
    assert_eq!(mind.asked(), 4);
    assert!(!mind.asks()[3].contains("first-session-secret"));
    fixture.close().await;
}
