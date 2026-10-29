use super::*;
use magi_testkit::Mind;

fn catalog(fixture: &Fixture, mind: &Mind) {
    let native = std::fs::read_link(fixture.root.join("bin/melchior")).expect("native router");
    turns::synthetic(fixture, mind);
    let cards: Vec<Value> = ["one", "two", "unavailable"]
        .into_iter()
        .map(|name| {
            json!({
                "id":format!("fake/{name}"), "provider":"fake", "name":name,
                "api":"anthropic-messages", "context_window":200000, "max_output":4096,
                "reasons":true, "ready":name != "unavailable"
            })
        })
        .collect();
    let models = json!({"ok":true,"n":cards.len(),"result":cards});
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\nmodels) printf '%s\\n' '{models}' ;;\nask) exec '{}' \"$@\" ;;\n*) exec '{}' \"$@\" ;;\nesac\n",
        mind.program().display(),
        native.display()
    );
    let path = fixture.root.join("bin/melchior");
    std::fs::write(&path, script).expect("synthetic catalog");
    magi_model::scratch::executable_ready(&path);
}

async fn selected(fixture: &mut Fixture, id: u64, session: &Value, model: &str) -> Value {
    fixture
        .send(
            json!({"jsonrpc":"2.0","id":id,"method":"session/set_config_option",
        "params":{"sessionId":session,"configId":"model","value":model}}),
        )
        .await;
    let mut updated = false;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], id);
            if frame.get("result").is_some() {
                assert!(updated, "native change precedes acknowledgement");
                assert_eq!(frame["result"]["configOptions"][0]["currentValue"], model);
                let _: agent_client_protocol_schema::v1::SetSessionConfigOptionResponse =
                    serde_json::from_value(frame["result"].clone()).expect("schema response");
            } else {
                assert!(!updated, "refusal cannot report changed state");
            }
            return frame;
        }
        assert_eq!(frame["params"]["sessionId"], *session);
        assert_eq!(
            frame["params"]["update"]["sessionUpdate"],
            "config_option_update"
        );
        assert_eq!(
            frame["params"]["update"]["configOptions"][0]["currentValue"],
            model
        );
        assert!(!updated, "one native change");
        updated = true;
    }
}

async fn answer(fixture: &mut Fixture, id: u64, session: &Value) {
    turns::prompt(fixture, id, session, "which model").await;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], id);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            return;
        }
        assert_eq!(frame["params"]["sessionId"], *session);
    }
}

#[tokio::test]
async fn native_acp_model_selection_is_correlated_session_local_and_used_by_native_turns() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::answering("acp-model-switch", "native answer");
    catalog(&fixture, &mind);
    let first = fixture
        .call(
            2,
            "session/new",
            json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
        )
        .await;
    let _: agent_client_protocol_schema::v1::NewSessionResponse =
        serde_json::from_value(first["result"].clone()).expect("schema session");
    let option = &first["result"]["configOptions"][0];
    assert_eq!(option["currentValue"], "fake/one");
    assert_eq!(option["options"].as_array().expect("choices").len(), 3);
    let first = first["result"]["sessionId"].clone();
    let second = fixture
        .call(
            3,
            "session/new",
            json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
        )
        .await;
    let second = second["result"]["sessionId"].clone();
    assert!(
        selected(&mut fixture, 4, &first, "fake/two")
            .await
            .get("result")
            .is_some()
    );
    assert!(
        selected(&mut fixture, 4, &first, "fake/two")
            .await
            .get("result")
            .is_some()
    );
    answer(&mut fixture, 5, &first).await;
    answer(&mut fixture, 6, &second).await;
    let asks = mind.asks();
    assert_eq!(asks.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&asks[0]).expect("first ask")["model"],
        "fake/two"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&asks[1]).expect("second ask")["model"],
        "fake/one"
    );
    assert!(!fixture.root.join("d/magi/chosen.json").exists());
    assert!(!fixture.root.join("d/magi/chosen.lock").exists());
    let third = fixture
        .call(
            7,
            "session/new",
            json!({"cwd":fixture.root.join("a"),"mcpServers":[]}),
        )
        .await;
    assert_eq!(
        third["result"]["configOptions"][0]["currentValue"],
        "fake/one"
    );
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_model_refusal_keeps_the_previous_native_worker() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::answering("acp-model-refusal", "native answer");
    catalog(&fixture, &mind);
    let session = turns::session(&mut fixture).await;
    for (name, code) in [("fake/unavailable", -32000), ("fake/unknown", -32602)] {
        let response = selected(&mut fixture, 3, &session, name).await;
        assert_eq!(response["error"]["code"], code, "{response}");
    }
    assert_eq!(mind.asked(), 0);
    answer(&mut fixture, 4, &session).await;
    assert_eq!(
        serde_json::from_str::<Value>(&mind.asks()[0]).expect("ask")["model"],
        "fake/one"
    );
    assert!(!fixture.root.join("d/magi/chosen.json").exists());
    fixture.close().await;
}

#[tokio::test]
async fn native_acp_busy_model_change_does_not_block_cancellation_or_other_sessions() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = Mind::controlled("acp-model-busy");
    catalog(&fixture, &mind);
    let session = turns::session(&mut fixture).await;
    let second = fixture
        .call(
            3,
            "session/new",
            json!({"cwd":fixture.root.join("b"),"mcpServers":[]}),
        )
        .await["result"]["sessionId"]
        .clone();
    turns::prompt(&mut fixture, 4, &session, "ongoing").await;
    loop {
        let frame = fixture.read().await;
        if frame["params"]["update"]["content"]["text"] == "part-0" {
            break;
        }
        assert!(frame.get("id").is_none(), "{frame}");
    }
    let refusal = selected(&mut fixture, 5, &session, "fake/two").await;
    assert_eq!(refusal["error"]["code"], -32000);
    assert!(
        selected(&mut fixture, 6, &second, "fake/two")
            .await
            .get("result")
            .is_some()
    );
    fixture
        .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}))
        .await;
    loop {
        let frame = fixture.read().await;
        if frame.get("id").is_some() {
            assert_eq!(frame["id"], 4);
            assert_eq!(frame["result"]["stopReason"], "cancelled", "{frame}");
            break;
        }
    }
    assert!(
        selected(&mut fixture, 7, &session, "fake/two")
            .await
            .get("result")
            .is_some()
    );
    assert_eq!(mind.asked(), 1);
    fixture.close().await;
}
