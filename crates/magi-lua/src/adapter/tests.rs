//! The two protocols most of the catalog depends on.

use super::support::*;
use super::*;
use magi_model::{Content, Message, Role, StopReason, ThinkingLevel};

#[test]
fn every_builtin_protocol_registers() {
    let mut engine = engine_with_builtins().expect("builtins load");
    let apis = engine.apis();
    assert!(apis.contains(&"anthropic-messages".to_owned()), "{apis:?}");
    assert!(apis.contains(&"openai-completions".to_owned()), "{apis:?}");
}

#[test]
fn an_unregistered_protocol_is_refused_and_says_what_is_known() {
    let Err(error) = LuaAdapter::new(engine_with_builtins().expect("builtins"), "nonsense") else {
        panic!("an unregistered protocol must be refused")
    };
    assert!(error.contains("nonsense"), "{error}");
    assert!(error.contains("anthropic-messages"), "{error}");
}

// ------------------------------------------------------------ anthropic-messages

#[test]
fn anthropic_builds_its_endpoint_and_headers() {
    let a = adapter("anthropic-messages");
    assert_eq!(
        a.endpoint("https://api.anthropic.com/", &plain_model()),
        "https://api.anthropic.com/v1/messages"
    );
    let headers = a.headers(Some("secret"));
    assert!(headers.contains(&("x-api-key".into(), "secret".into())));
    assert!(!a.headers(None).iter().any(|(n, _)| n == "x-api-key"));
}

#[test]
fn anthropic_streams_and_caps_its_tokens() {
    let body = adapter("anthropic-messages").request(
        &plain_model(),
        &plain_context(),
        &Options {
            max_tokens: Some(999_999),
            ..Options::default()
        },
    );
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 8192);
    assert_eq!(body["messages"][0]["content"][0]["text"], "hi");
}

#[test]
fn anthropic_merges_consecutive_same_role_messages() {
    let context = Context {
        messages: vec![
            Message::user("first"),
            Message {
                role: Role::Tool,
                content: vec![Content::ToolResult {
                    id: "t1".into(),
                    name: "read".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
                stop_reason: None,
                usage: None,
                error: None,
            },
        ],
        ..Context::default()
    };
    let body = adapter("anthropic-messages").request(&plain_model(), &context, &Options::default());
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "the API refuses two user turns in a row");
    assert_eq!(messages[0]["content"].as_array().expect("blocks").len(), 2);
}

#[test]
fn anthropic_drops_a_thinking_block_that_lost_its_signature() {
    let context = Context {
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![
                Content::Thinking {
                    thinking: "lost".into(),
                    signature: None,
                },
                Content::Text {
                    text: "kept".into(),
                    signature: None,
                },
            ],
            stop_reason: None,
            usage: None,
            error: None,
        }],
        ..Context::default()
    };
    let body = adapter("anthropic-messages").request(&plain_model(), &context, &Options::default());
    let blocks = body["messages"][0]["content"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["text"], "kept");
}

#[test]
fn anthropic_leaves_room_for_a_response_when_asked_to_think() {
    let mut small = plain_model();
    small.max_tokens = 2048;
    let body = adapter("anthropic-messages").request(
        &small,
        &plain_context(),
        &Options {
            thinking: Some(ThinkingLevel::Max),
            ..Options::default()
        },
    );
    let budget = body["thinking"]["budget_tokens"]
        .as_u64()
        .expect("a budget");
    assert!(budget < small.max_tokens, "all of it leaves an empty turn");
}

#[test]
fn anthropic_thinking_off_omits_the_field() {
    let body = adapter("anthropic-messages").request(
        &plain_model(),
        &plain_context(),
        &Options {
            thinking: Some(ThinkingLevel::Off),
            ..Options::default()
        },
    );
    assert!(
        body.get("thinking").is_none(),
        "off is not a budget of zero"
    );
}

#[test]
fn anthropic_streams_text_thinking_and_a_signature() {
    let deltas = stream(
        &adapter("anthropic-messages"),
        &[
            (
                "content_block_start",
                r#"{"content_block":{"type":"thinking"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"thinking_delta","thinking":"why"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"signature_delta","signature":"sig"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"text_delta","text":"Hi"}}"#,
            ),
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
fn anthropic_streams_a_tool_call() {
    let deltas = stream(
        &adapter("anthropic-messages"),
        &[
            (
                "content_block_start",
                r#"{"content_block":{"type":"tool_use","id":"t1","name":"read"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}"#,
            ),
        ],
    );
    assert_eq!(
        deltas,
        vec![
            Delta::ToolCallStart {
                id: "t1".into(),
                name: "read".into()
            },
            Delta::ToolCallArgs("{\"a\":1}".into()),
        ]
    );
}

#[test]
fn anthropic_completes_its_usage_at_the_end() {
    let deltas = stream(
        &adapter("anthropic-messages"),
        &[
            (
                "message_start",
                r#"{"message":{"usage":{"input_tokens":10,"cache_read_input_tokens":90}}}"#,
            ),
            (
                "message_delta",
                r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            ),
        ],
    );
    let Delta::Usage(last) = deltas[1] else {
        panic!("expected usage, got {:?}", deltas[1]);
    };
    assert_eq!(last.output, 5);
    assert_eq!(last.input, 10, "the earlier counts survive");
    assert_eq!(deltas[2], Delta::Stop(StopReason::EndTurn));
}

#[test]
fn anthropic_maps_a_truncated_turn_to_length() {
    let deltas = stream(
        &adapter("anthropic-messages"),
        &[("message_delta", r#"{"delta":{"stop_reason":"max_tokens"}}"#)],
    );
    assert_eq!(deltas, vec![Delta::Stop(StopReason::Length)]);
}

// ------------------------------------------------------------ openai-completions

#[test]
fn completions_builds_its_endpoint_and_bearer() {
    let a = adapter("openai-completions");
    assert_eq!(
        a.endpoint("https://api.groq.com/openai/v1", &plain_model()),
        "https://api.groq.com/openai/v1/chat/completions"
    );
    assert!(
        a.headers(Some("k"))
            .contains(&("authorization".into(), "Bearer k".into()))
    );
}

#[test]
fn completions_uses_the_conservative_token_field_by_default() {
    let body = adapter("openai-completions").request(
        &plain_model(),
        &plain_context(),
        &Options::default(),
    );
    assert_eq!(body["max_tokens"], 8192);
    assert!(body.get("max_completion_tokens").is_none());
    assert!(body.get("store").is_none(), "an unknown field is a 400");
}

#[test]
fn completions_honours_a_declared_dialect() {
    let mut m = plain_model();
    m.compat = Some(magi_provider::compat::Compat {
        max_tokens_field: Some(magi_provider::compat::MaxTokensField::MaxCompletionTokens),
        supports_developer_role: Some(true),
        ..magi_provider::compat::Compat::default()
    });
    let context = Context {
        system: Some("be brief".into()),
        ..plain_context()
    };
    let body = adapter("openai-completions").request(&m, &context, &Options::default());
    assert_eq!(body["max_completion_tokens"], 8192);
    assert_eq!(body["messages"][0]["role"], "developer");
}

#[test]
fn completions_puts_a_tool_result_in_its_own_role() {
    let context = Context {
        messages: vec![Message {
            role: Role::Tool,
            content: vec![Content::ToolResult {
                id: "t1".into(),
                name: "read".into(),
                content: "ok".into(),
                is_error: false,
            }],
            stop_reason: None,
            usage: None,
            error: None,
        }],
        ..Context::default()
    };
    let body = adapter("openai-completions").request(&plain_model(), &context, &Options::default());
    assert_eq!(body["messages"][0]["role"], "tool");
    assert_eq!(body["messages"][0]["tool_call_id"], "t1");
}

#[test]
fn completions_streams_text_and_reasoning() {
    let deltas = stream(
        &adapter("openai-completions"),
        &[
            ("", r#"{"choices":[{"delta":{"reasoning_content":"why"}}]}"#),
            ("", r#"{"choices":[{"delta":{"content":"Hi"}}]}"#),
        ],
    );
    assert_eq!(
        deltas,
        vec![Delta::Thinking("why".into()), Delta::Text("Hi".into())]
    );
}

#[test]
fn completions_announces_a_tool_call_once_and_streams_its_arguments() {
    // Only the first chunk of each call carries its id and name; the rest are arguments.
    let deltas = stream(
        &adapter("openai-completions"),
        &[
            (
                "",
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"t1","function":{"name":"read","arguments":"{\"a"}}]}}]}"#,
            ),
            (
                "",
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\":1}"}}]}}]}"#,
            ),
        ],
    );
    assert_eq!(
        deltas,
        vec![
            Delta::ToolCallStart {
                id: "t1".into(),
                name: "read".into()
            },
            Delta::ToolCallArgs("{\"a".into()),
            Delta::ToolCallArgs("\":1}".into()),
        ]
    );
}

#[test]
fn completions_maps_its_finish_reasons() {
    for (wire, expected) in [
        ("stop", StopReason::EndTurn),
        ("length", StopReason::Length),
        ("tool_calls", StopReason::ToolUse),
    ] {
        let deltas = stream(
            &adapter("openai-completions"),
            &[(
                "",
                &format!(r#"{{"choices":[{{"finish_reason":"{wire}"}}]}}"#),
            )],
        );
        assert_eq!(deltas, vec![Delta::Stop(expected)], "{wire}");
    }
}

#[test]
fn completions_ends_on_the_sentinel_when_no_finish_reason_arrived() {
    // Without this a clean end is indistinguishable from a dropped connection, which is
    // this dialect's one genuine oddity.
    let deltas = stream(
        &adapter("openai-completions"),
        &[
            ("", r#"{"choices":[{"delta":{"content":"Hi"}}]}"#),
            ("", "[DONE]"),
        ],
    );
    assert_eq!(deltas.last(), Some(&Delta::Stop(StopReason::EndTurn)));
}

#[test]
fn completions_does_not_stop_twice() {
    let deltas = stream(
        &adapter("openai-completions"),
        &[
            ("", r#"{"choices":[{"finish_reason":"stop"}]}"#),
            ("", "[DONE]"),
        ],
    );
    let stops = deltas
        .iter()
        .filter(|d| matches!(d, Delta::Stop(_)))
        .count();
    assert_eq!(stops, 1, "the sentinel must not repeat a stop already sent");
}

#[test]
fn completions_separates_cached_tokens_from_fresh_ones() {
    let deltas = stream(
        &adapter("openai-completions"),
        &[(
            "",
            r#"{"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":90}}}"#,
        )],
    );
    let Delta::Usage(usage) = deltas[0] else {
        panic!("expected usage");
    };
    assert_eq!(usage.cache_read, 90);
    assert_eq!(usage.input, 10, "cached tokens are not billed as input");
    assert_eq!(usage.output, 7);
}

#[test]
fn a_malformed_payload_is_ignored_rather_than_fatal() {
    for name in ["anthropic-messages", "openai-completions"] {
        assert!(
            stream(&adapter(name), &[("", "not json")]).is_empty(),
            "{name}"
        );
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// Options asking for one small object back.
    fn asking_for_a_shape() -> Options {
        Options {
            schema: Some(magi_provider::api::Schema {
                name: "answer".into(),
                schema: serde_json::json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } },
                    "required": ["ok"],
                }),
            }),
            ..Options::default()
        }
    }

    #[test]
    fn completions_asks_for_a_json_schema_response_format() {
        let body = adapter("openai-completions").request(
            &plain_model(),
            &plain_context(),
            &asking_for_a_shape(),
        );
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["response_format"]["json_schema"]["name"], "answer");
    }

    #[test]
    fn responses_puts_it_under_text_format() {
        let body = adapter("openai-responses").request(
            &plain_model(),
            &plain_context(),
            &asking_for_a_shape(),
        );
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["name"], "answer");
    }

    #[test]
    fn google_puts_it_on_the_generation_config() {
        let body = adapter("google-generative-ai").request(
            &plain_model(),
            &plain_context(),
            &asking_for_a_shape(),
        );
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert!(body["generationConfig"]["responseSchema"].is_object());
    }

    #[test]
    fn anthropic_forces_a_single_tool_because_it_has_no_response_format() {
        // The idiom there: one tool whose input schema is the shape wanted, and `tool_choice`
        // naming it, so the answer arrives as a tool call rather than as text.
        let body = adapter("anthropic-messages").request(
            &plain_model(),
            &plain_context(),
            &asking_for_a_shape(),
        );
        assert_eq!(body["tool_choice"]["type"], "tool");
        assert_eq!(body["tool_choice"]["name"], "answer");
        assert_eq!(body["tools"][0]["name"], "answer");
        assert!(body["tools"][0]["input_schema"].is_object());
    }

    #[test]
    fn no_schema_asked_for_means_no_field_added() {
        // A request that did not ask for a shape must look exactly as it did before.
        let body = adapter("openai-completions").request(
            &plain_model(),
            &plain_context(),
            &Options::default(),
        );
        assert!(body.get("response_format").is_none());
    }
}
