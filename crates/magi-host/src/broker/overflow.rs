use super::*;
use magi_proto::ask::overflow::Kind;
use magi_testkit::Mind;
use serde_json::json;

#[tokio::test]
async fn actual_peer_optional_detail_never_loses_terminal_failure_or_changes_request() {
    for (index, (why, metadata, expected)) in [
        ("overflow", json!(null), None),
        (
            "overflow",
            json!({"kind":"input","prompt_tokens":250000}),
            Some(Kind::Input),
        ),
        (
            "overflow",
            json!({"kind":"output_budget","limit_tokens":16000}),
            Some(Kind::OutputBudget),
        ),
        ("overflow", json!({"kind":"future"}), Some(Kind::Unknown)),
        ("overflow", json!({"kind":"input","limit_tokens":-1}), None),
        ("overflow", json!(["future"]), None),
        ("auth", json!({"kind":"output_budget"}), None),
        ("throttle", json!({"kind":"output_budget"}), None),
    ]
    .into_iter()
    .enumerate()
    {
        let response =
            json!({"event":"failed","message":"exact failure","why":why,"overflow":metadata})
                .to_string();
        let mind = Mind::saying(&format!("overflow-wire-{index}"), &[&response]);
        let context = Context {
            system: Some(format!("overflow contract {index}")),
            messages: vec![magi_model::Message::user("unchanged")],
            ..Default::default()
        };
        let wants = Wants::default();
        let mut deltas = Vec::new();
        let mut retries = Vec::new();
        let failure = asking(
            Asked {
                program: mind.program().to_str().expect("program"),
                environ: &Default::default(),
                model: "fake/overflow",
                context: &context,
                wants: &wants,
            },
            |delta| deltas.push(delta),
            |retry| retries.push(retry),
            std::time::Duration::from_secs(2),
        )
        .await
        .expect_err("terminal refusal");
        assert_eq!(failure.message, "exact failure");
        assert_eq!(serde_json::to_value(failure.why).expect("class"), why);
        assert_eq!(failure.overflow.map(|detail| detail.kind), expected);
        assert!(deltas.is_empty());
        assert!(retries.is_empty());
        let asks = mind.asks();
        assert_eq!(asks.len(), 1);
        assert_eq!(
            serde_json::from_str::<Ask>(&asks[0]).expect("ask").context,
            context
        );
    }
}
