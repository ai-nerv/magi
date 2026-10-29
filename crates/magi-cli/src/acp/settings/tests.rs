#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use tokio::io::AsyncWriteExt;

fn choice(name: &str) -> ModelChoice {
    serde_json::from_value(json!({"name":name,"context_window":200000,"requirement":""}))
        .expect("choice")
}

fn model(name: &str) -> Option<ModelInfo> {
    Some(ModelInfo {
        name: name.into(),
        context_window: 200000,
    })
}

fn settings() -> Settings {
    Settings::new(
        model("fake/one"),
        vec![choice("fake/one"), choice("fake/two")],
    )
    .expect("settings")
}

async fn lease() -> Option<intake::Item> {
    let (mut input, reader) = tokio::io::duplex(256);
    let mut intake = intake::Intake::new(reader);
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"session/set_config_option\"}\n")
        .await
        .expect("request");
    let item = intake.incoming.recv().await;
    intake.finish().await;
    item
}

#[test]
fn acp_model_options_use_native_state_and_official_schema() {
    let settings = settings();
    let options = serde_json::to_value(settings.options()).expect("options");
    assert_eq!(options[0]["id"], "model");
    assert_eq!(options[0]["type"], "select");
    assert_eq!(options[0]["category"], "model");
    assert_eq!(options[0]["currentValue"], "fake/one");
    assert_eq!(options[0]["options"].as_array().expect("choices").len(), 2);
    let notification = settings.notification("owned");
    let _: agent_client_protocol_schema::v1::SessionNotification =
        serde_json::from_value(notification["params"].clone()).expect("schema update");
}

#[test]
fn acp_model_options_do_not_invent_an_unconfigured_model() {
    let settings = Settings::new(None, vec![choice("fake/one")]).expect("unconfigured");
    assert!(settings.options().is_empty());
    let settings = Settings::new(model("fake/old"), vec![]).expect("retired current");
    let options = serde_json::to_value(settings.options()).expect("options");
    assert_eq!(options[0]["options"][0]["value"], "fake/old");
}

#[test]
fn acp_model_request_is_strict_and_bounded() {
    let valid = json!({"sessionId":"owned","configId":"model","value":"fake/two"});
    assert_eq!(
        parse(valid.clone()).expect("valid"),
        ("owned".into(), "fake/two".into())
    );
    let mut typed = valid.clone();
    typed["type"] = json!("select");
    assert!(parse(typed).is_ok());
    for (key, value) in [
        ("type", json!("boolean")),
        ("type", json!("unknown")),
        ("type", Value::Null),
        ("value", json!(true)),
        ("value", json!("")),
        ("value", json!("x".repeat(NAME + 1))),
        ("value", json!("x\ny")),
        ("configId", json!("mode")),
        ("sessionId", json!("")),
        ("unknown", json!(true)),
        ("_meta", json!({"long":"x".repeat(16384)})),
    ] {
        let mut request = valid.clone();
        request[key] = value;
        assert!(parse(request).is_err(), "{key}");
    }
}

#[test]
fn acp_model_catalog_limits_fail_without_truncation() {
    assert!(Settings::new(model("fake/one"), vec![choice("same"); 2]).is_err());
    assert!(Settings::new(None, vec![choice("x"); MODELS + 1]).is_err());
    assert!(Settings::new(model(&"x".repeat(NAME + 1)), vec![]).is_err());
    let mut too_long = choice("fake/one");
    too_long.requirement = "x".repeat(4097);
    assert!(Settings::new(None, vec![too_long]).is_err());
    let big = (0..1024)
        .map(|n| {
            let mut choice = choice(&format!("fake/{n}"));
            choice.requirement = "x".repeat(4096);
            choice
        })
        .collect();
    assert!(Settings::new(None, big).is_err());
}

#[tokio::test]
async fn acp_model_success_requires_correlated_native_receipt_and_state() {
    let mut settings = settings();
    let mut lease = lease().await;
    let command = settings
        .select(RequestId::Number(7), "fake/two".into(), &mut lease)
        .expect("select");
    assert_eq!(
        command,
        UiCommand::SelectModel {
            request: 1,
            name: "fake/two".into()
        }
    );
    assert!(lease.is_none());
    assert!(settings.busy());
    assert!(settings.deadline().is_some());
    assert!(settings.response(1, None).is_err());
    settings.model(model("fake/two")).expect("native change");
    assert!(settings.busy());
    assert!(settings.response(2, None).is_err());
    let response = settings.response(1, None).expect("receipt");
    assert_eq!(response["id"], 7);
    assert_eq!(
        response["result"]["configOptions"][0]["currentValue"],
        "fake/two"
    );
    let _: SetSessionConfigOptionResponse =
        serde_json::from_value(response["result"].clone()).expect("schema response");
    assert!(settings.busy(), "lease remains held through response write");
    settings.release();
    assert!(!settings.busy());
    assert!(settings.deadline().is_none());
    assert!(settings.response(1, None).is_err());
}

#[tokio::test]
async fn acp_model_refusal_preserves_state_and_same_value_is_correlated() {
    let mut settings = settings();
    settings
        .select(RequestId::Number(7), "fake/two".into(), &mut lease().await)
        .expect("select");
    let response = settings.response(1, Some("busy".into())).expect("refusal");
    assert!(response.get("error").is_some());
    assert_eq!(settings.current.as_deref(), Some("fake/one"));
    settings.release();
    let command = settings
        .select(RequestId::Number(7), "fake/one".into(), &mut lease().await)
        .expect("same value");
    assert_eq!(
        command,
        UiCommand::SelectModel {
            request: 2,
            name: "fake/one".into()
        }
    );
    assert!(settings.response(1, None).is_err());
    assert!(
        settings
            .response(2, None)
            .expect("new receipt")
            .get("result")
            .is_some()
    );
}

#[tokio::test]
async fn acp_model_invalid_busy_and_exhausted_requests_retain_their_leases() {
    let mut settings = settings();
    let mut item = lease().await;
    assert!(
        settings
            .select(RequestId::Number(7), "unknown".into(), &mut item)
            .is_err()
    );
    assert!(item.is_some());
    settings
        .select(RequestId::Number(7), "fake/two".into(), &mut item)
        .expect("select");
    let mut rejected = lease().await;
    assert!(
        settings
            .select(RequestId::Number(7), "fake/one".into(), &mut rejected)
            .is_err()
    );
    assert!(rejected.is_some());
    settings.release();
    settings.next = u64::MAX;
    assert!(
        settings
            .select(RequestId::Number(7), "fake/one".into(), &mut rejected)
            .is_err()
    );
    assert!(rejected.is_some());
    assert!(!settings.busy());
}

#[tokio::test(start_paused = true)]
async fn acp_model_expired_receipt_cannot_win_a_dispatch_race_with_the_timer() {
    let mut settings = settings();
    settings
        .select(RequestId::Number(7), "fake/two".into(), &mut lease().await)
        .expect("select");
    settings.model(model("fake/two")).expect("native update");
    tokio::time::advance(transport::DEADLINE).await;
    assert!(settings.response(1, None).is_err());
    assert!(settings.response(1, Some("busy".into())).is_err());
    assert!(settings.busy(), "cleanup retains the lease");
}
