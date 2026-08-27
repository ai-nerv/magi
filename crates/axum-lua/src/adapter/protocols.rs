//! The protocol families: the Responses hostings, and Google's two doors.

use super::support::*;
use super::*;
use axum_model::{Context, Message, StopReason};
use axum_provider::model::Api;

#[test]
fn the_responses_family_shares_one_body_and_differs_only_at_the_door() {
    let bodies: Vec<serde_json::Value> = [
        "openai-responses",
        "azure-openai-responses",
        "openai-codex-responses",
    ]
    .iter()
    .map(|name| adapter(name).request(&plain_model(), &plain_context(), &Options::default()))
    .collect();
    assert_eq!(bodies[0], bodies[1], "azure serves the same protocol");
    assert_eq!(bodies[0], bodies[2], "so does the subscription backend");

    assert_eq!(
        adapter("openai-responses").endpoint("https://api.openai.com/v1", &plain_model()),
        "https://api.openai.com/v1/responses"
    );
    assert!(
        adapter("azure-openai-responses")
            .endpoint("https://x.openai.azure.com", &plain_model())
            .contains("/deployments/m-1/responses"),
    );
    assert!(
        adapter("azure-openai-responses")
            .headers(Some("k"))
            .contains(&("api-key".into(), "k".into())),
        "azure authenticates by header name, not bearer"
    );
}

#[test]
fn responses_names_its_content_parts_by_direction() {
    // What the model produced is `output_text`; what it was given is `input_text`, and
    // swapping them is rejected.
    let context = Context {
        messages: vec![Message::user("asked"), Message::assistant("answered")],
        ..Context::default()
    };
    let body = adapter("openai-responses").request(&plain_model(), &context, &Options::default());
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
}

#[test]
fn responses_streams_text_reasoning_and_a_call() {
    let deltas = stream(
        &adapter("openai-responses"),
        &[
            (
                "",
                r#"{"type":"response.reasoning_text.delta","delta":"why"}"#,
            ),
            ("", r#"{"type":"response.output_text.delta","delta":"Hi"}"#),
            (
                "",
                r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"c1","name":"read"}}"#,
            ),
            (
                "",
                r#"{"type":"response.function_call_arguments.delta","delta":"{\"a\":1}"}"#,
            ),
        ],
    );
    assert_eq!(
        deltas,
        vec![
            Delta::Thinking("why".into()),
            Delta::Text("Hi".into()),
            Delta::ToolCallStart {
                id: "c1".into(),
                name: "read".into()
            },
            Delta::ToolCallArgs("{\"a\":1}".into()),
        ]
    );
}

#[test]
fn a_response_that_ran_out_of_room_is_length_not_a_clean_finish() {
    // The case that must poison the turn's tool calls.
    let deltas = stream(
        &adapter("openai-responses"),
        &[(
            "",
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )],
    );
    assert_eq!(deltas.last(), Some(&Delta::Stop(StopReason::Length)));
}

#[test]
fn a_completed_response_carrying_a_call_is_waiting_on_it() {
    let deltas = stream(
        &adapter("openai-responses"),
        &[(
            "",
            r#"{"type":"response.completed","response":{"output":[{"type":"function_call"}]}}"#,
        )],
    );
    assert_eq!(deltas.last(), Some(&Delta::Stop(StopReason::ToolUse)));
}

#[test]
fn google_calls_the_models_turn_model_and_merges_consecutive_ones() {
    let context = Context {
        messages: vec![Message::assistant("one"), Message::assistant("two")],
        ..Context::default()
    };
    let body =
        adapter("google-generative-ai").request(&plain_model(), &context, &Options::default());
    let contents = body["contents"].as_array().expect("contents");
    assert_eq!(contents.len(), 1, "turns must alternate");
    assert_eq!(contents[0]["role"], "model");
    assert_eq!(contents[0]["parts"].as_array().expect("parts").len(), 2);
}

#[test]
fn google_streams_over_sse_only_when_asked() {
    // Without `alt=sse` the endpoint answers one array at the end, which looks like a very slow
    // model rather than a configuration mistake.
    let url = adapter("google-generative-ai").endpoint(
        "https://generativelanguage.googleapis.com/v1beta",
        &plain_model(),
    );
    assert!(url.contains(":streamGenerateContent?alt=sse"), "{url}");
    assert!(
        adapter("google-vertex")
            .endpoint(
                "https://europe-west1-aiplatform.googleapis.com/v1/projects/p",
                &plain_model()
            )
            .contains("/publishers/google/models/m-1:streamGenerateContent"),
    );
}

#[test]
fn google_reports_a_thought_and_its_signature() {
    let deltas = stream(
        &adapter("google-generative-ai"),
        &[(
            "",
            r#"{"candidates":[{"content":{"parts":[{"text":"why","thought":true,"thoughtSignature":"sig"},{"text":"Hi"}]}}]}"#,
        )],
    );
    assert_eq!(
        deltas,
        vec![
            Delta::Thinking("why".into()),
            Delta::Signature("sig".into()),
            Delta::Text("Hi".into()),
        ]
    );
}

#[test]
fn google_reports_a_call_as_one_whole_set_of_arguments() {
    // Unlike the other dialects, Google does not stream arguments in pieces.
    let deltas = stream(
        &adapter("google-generative-ai"),
        &[(
            "",
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"a"}}}]},"finishReason":"STOP"}]}"#,
        )],
    );
    assert_eq!(
        deltas[0],
        Delta::ToolCallStart {
            id: "read".into(),
            name: "read".into()
        }
    );
    assert!(matches!(deltas[1], Delta::ToolCallArgs(_)));
    assert_eq!(
        deltas.last(),
        Some(&Delta::Stop(StopReason::ToolUse)),
        "a turn that produced a call has not ended"
    );
}

#[test]
fn google_separates_cached_tokens_from_fresh_ones() {
    let deltas = stream(
        &adapter("google-generative-ai"),
        &[(
            "",
            r#"{"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":7,"cachedContentTokenCount":90}}"#,
        )],
    );
    let Delta::Usage(usage) = deltas[0] else {
        panic!("expected usage");
    };
    assert_eq!(usage.cache_read, 90);
    assert_eq!(usage.input, 10);
}

#[test]
fn pi_messages_reports_a_signature_when_its_block_closes() {
    let deltas = stream(
        &adapter("pi-messages"),
        &[
            ("", r#"{"type":"thinking_delta","delta":"why"}"#),
            ("", r#"{"type":"thinking_end","signature":"sig"}"#),
            ("", r#"{"type":"text_delta","delta":"Hi"}"#),
        ],
    );
    assert_eq!(
        deltas,
        vec![
            Delta::Thinking("why".into()),
            Delta::Signature("sig".into()),
            Delta::Text("Hi".into()),
        ]
    );
}

#[test]
fn every_registered_protocol_survives_a_malformed_payload() {
    let mut engine = engine_with_builtins().expect("builtins");
    for name in engine.apis() {
        assert!(
            stream(&adapter(&name), &[("", "not json")]).is_empty(),
            "{name}"
        );
    }
}

#[test]
fn every_protocol_is_either_described_or_a_stated_gap() {
    // The invariant that keeps a silent absence from ever being possible.
    let mut engine = engine_with_builtins().expect("builtins");
    let spoken = engine.apis();
    for api in Api::all() {
        let name = api.as_str();
        assert!(
            spoken.iter().any(|s| s == name) || why_unspoken(name).is_some(),
            "{name} is neither described nor explained"
        );
    }
}

#[test]
fn a_stated_gap_is_not_also_described() {
    let mut engine = engine_with_builtins().expect("builtins");
    let spoken = engine.apis();
    for (name, _) in UNSPOKEN {
        assert!(
            !spoken.iter().any(|s| s == name),
            "{name} is described; remove it from the gap list"
        );
    }
}

#[test]
fn a_stated_gap_explains_itself_rather_than_restating_the_name() {
    for (name, why) in UNSPOKEN {
        assert!(why.len() > 40, "{name}'s reason says too little");
        assert!(why.contains("because") || why.contains("so"), "{name}");
    }
}

#[test]
fn axum_speaks_eight_of_the_nine_protocols_it_knows() {
    let mut engine = engine_with_builtins().expect("builtins");
    assert_eq!(engine.apis().len(), 8);
    assert_eq!(Api::all().len(), 9);
}
