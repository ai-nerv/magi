//! A wire protocol, described in Lua.
//!
//! The only implementation of [`Adapter`] there is. `axum-provider` owns the contract and moves
//! the bytes; every protocol — Anthropic's Messages, OpenAI's Completions, the eight others —
//! is a table of functions in `lua/apis/*.lua`, registered like anything else.
//!
//! A protocol is a description. Describing one should not need a rebuild, and a person with a
//! private endpoint that speaks something slightly different should be able to say so.

use crate::Engine;
use axum_model::Context;
use axum_provider::api::{Adapter, Delta, Options, StreamState};
use axum_provider::model::Model;
use axum_provider::sse;
use std::cell::RefCell;

/// One registered protocol, driven through the VM.
pub struct LuaAdapter {
    /// The VM holding the description. Borrowed mutably per call, so it is not shared.
    engine: RefCell<Engine>,
    /// Which protocol, by the name it registered under.
    name: String,
}

impl LuaAdapter {
    /// Take ownership of an engine and speak `name` through it.
    ///
    /// # Errors
    /// When nothing registered under that name.
    pub fn new(mut engine: Engine, name: &str) -> Result<Self, String> {
        let known = engine.apis();
        if !known.iter().any(|a| a == name) {
            return Err(format!(
                "no protocol named {name:?} is registered; axum knows {}",
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            ));
        }
        Ok(Self {
            engine: RefCell::new(engine),
            name: name.to_owned(),
        })
    }

    fn call(&self, method: &str, args: &[serde_json::Value]) -> Option<serde_json::Value> {
        self.engine.borrow_mut().call_api(&self.name, method, args)
    }
}

impl Adapter for LuaAdapter {
    fn endpoint(&self, base_url: &str, model: &Model) -> String {
        self.call(
            "endpoint",
            &[
                serde_json::json!(base_url.trim_end_matches('/')),
                serde_json::to_value(model).unwrap_or_default(),
            ],
        )
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| base_url.to_owned())
    }

    fn headers(&self, key: Option<&str>) -> Vec<(String, String)> {
        // Content type is set here rather than in every protocol: all ten post JSON, and a
        // description that had to say so would be repeating the transport's own decision.
        let mut out = vec![("content-type".to_owned(), "application/json".to_owned())];
        let Some(fields) = self
            .call("headers", &[serde_json::json!(key)])
            .and_then(|v| v.as_object().cloned())
        else {
            return out;
        };
        for (name, value) in fields {
            if let Some(value) = value.as_str() {
                out.push((name, value.to_owned()));
            }
        }
        out
    }

    fn request(&self, model: &Model, context: &Context, options: &Options) -> serde_json::Value {
        self.call(
            "request",
            &[
                serde_json::to_value(model).unwrap_or_default(),
                serde_json::to_value(context).unwrap_or_default(),
                serde_json::to_value(options).unwrap_or_default(),
            ],
        )
        .unwrap_or(serde_json::Value::Null)
    }

    fn on_event(&self, state: &mut StreamState, event: &sse::Event) -> Vec<Delta> {
        let answer = self.call(
            "on_event",
            &[
                serde_json::json!({ "scratch": state.scratch, "usage": state.usage }),
                serde_json::json!({ "name": event.name, "data": event.data }),
            ],
        );
        let Some(answer) = answer else {
            return Vec::new();
        };

        // The protocol hands back what it wants remembered and what the caller should be told.
        // Remembering is the adapter's business; this only carries it between events.
        if let Some(scratch) = answer.get("scratch") {
            state.scratch = scratch.clone();
        }
        if let Some(usage) = answer
            .get("usage")
            .and_then(|u| serde_json::from_value(u.clone()).ok())
        {
            state.usage = usage;
        }
        answer
            .get("deltas")
            .and_then(|d| d.as_array())
            .map(|deltas| deltas.iter().filter_map(delta_from_json).collect())
            .unwrap_or_default()
    }
}

/// One delta, as a protocol described it.
///
/// An unrecognised kind is dropped rather than fatal: a protocol may learn to report something
/// this build has no vocabulary for, and losing that is better than losing the turn.
fn delta_from_json(value: &serde_json::Value) -> Option<Delta> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Some(match value.get("kind").and_then(|k| k.as_str())? {
        "text" => Delta::Text(text("text")),
        "thinking" => Delta::Thinking(text("thinking")),
        "signature" => Delta::Signature(text("signature")),
        "tool_call_start" => Delta::ToolCallStart {
            id: text("id"),
            name: text("name"),
        },
        "tool_call_args" => Delta::ToolCallArgs(text("arguments")),
        "usage" => Delta::Usage(serde_json::from_value(value.get("usage")?.clone()).ok()?),
        "stop" => Delta::Stop(serde_json::from_value(value.get("reason")?.clone()).ok()?),
        _ => return None,
    })
}

/// The protocol descriptions axum ships.
///
/// Compiled in so a fresh install speaks something, and registered through the same registrar a
/// user's own file would use — a private protocol is an extra file, not a fork.
pub const BUILTIN: &[(&str, &str)] = &[
    (
        "anthropic-messages",
        include_str!("../lua/apis/anthropic-messages.lua"),
    ),
    (
        "openai-completions",
        include_str!("../lua/apis/openai-completions.lua"),
    ),
];

/// An engine with every built-in protocol registered.
pub fn engine_with_builtins() -> Result<Engine, crate::LuaError> {
    let mut engine = Engine::new();
    for (name, source) in BUILTIN {
        engine.run(source, name)?;
    }
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_model::{Content, Cost, Message, Role, StopReason, ThinkingLevel};
    use axum_provider::model::{Api, Modality};
    use std::collections::BTreeMap;

    fn model(api: Api) -> Model {
        Model {
            id: "m-1".into(),
            name: "M".into(),
            provider: "p".into(),
            api,
            reasoning: true,
            input: vec![Modality::Text],
            context_window: 200_000,
            max_tokens: 8192,
            cost: Cost::default(),
            thinking: BTreeMap::new(),
            compat: None,
        }
    }

    fn adapter(name: &str) -> LuaAdapter {
        LuaAdapter::new(engine_with_builtins().expect("builtins load"), name)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// Fold recorded events and collect what a caller would be told.
    fn stream(adapter: &LuaAdapter, events: &[(&str, &str)]) -> Vec<Delta> {
        let mut state = StreamState::default();
        events
            .iter()
            .flat_map(|(name, data)| {
                adapter.on_event(
                    &mut state,
                    &sse::Event {
                        name: (*name).to_owned(),
                        data: (*data).to_owned(),
                    },
                )
            })
            .collect()
    }

    fn context() -> Context {
        Context {
            messages: vec![Message::user("hi")],
            ..Context::default()
        }
    }

    #[test]
    fn every_builtin_protocol_registers() {
        let mut engine = engine_with_builtins().expect("builtins load");
        let apis = engine.apis();
        assert!(apis.contains(&"anthropic-messages".to_owned()), "{apis:?}");
        assert!(apis.contains(&"openai-completions".to_owned()), "{apis:?}");
    }

    #[test]
    fn an_unregistered_protocol_is_refused_and_says_what_is_known() {
        let Err(error) = LuaAdapter::new(engine_with_builtins().expect("builtins"), "nonsense")
        else {
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
            a.endpoint("https://api.anthropic.com/", &model(Api::AnthropicMessages)),
            "https://api.anthropic.com/v1/messages"
        );
        let headers = a.headers(Some("secret"));
        assert!(headers.contains(&("x-api-key".into(), "secret".into())));
        assert!(!a.headers(None).iter().any(|(n, _)| n == "x-api-key"));
    }

    #[test]
    fn anthropic_streams_and_caps_its_tokens() {
        let body = adapter("anthropic-messages").request(
            &model(Api::AnthropicMessages),
            &context(),
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
        let body = adapter("anthropic-messages").request(
            &model(Api::AnthropicMessages),
            &context,
            &Options::default(),
        );
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
        let body = adapter("anthropic-messages").request(
            &model(Api::AnthropicMessages),
            &context,
            &Options::default(),
        );
        let blocks = body["messages"][0]["content"].as_array().expect("blocks");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["text"], "kept");
    }

    #[test]
    fn anthropic_leaves_room_for_a_response_when_asked_to_think() {
        let mut small = model(Api::AnthropicMessages);
        small.max_tokens = 2048;
        let body = adapter("anthropic-messages").request(
            &small,
            &context(),
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
            &model(Api::AnthropicMessages),
            &context(),
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
            a.endpoint(
                "https://api.groq.com/openai/v1",
                &model(Api::OpenAiCompletions)
            ),
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
            &model(Api::OpenAiCompletions),
            &context(),
            &Options::default(),
        );
        assert_eq!(body["max_tokens"], 8192);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.get("store").is_none(), "an unknown field is a 400");
    }

    #[test]
    fn completions_honours_a_declared_dialect() {
        let mut m = model(Api::OpenAiCompletions);
        m.compat = Some(axum_provider::compat::Compat {
            max_tokens_field: Some(axum_provider::compat::MaxTokensField::MaxCompletionTokens),
            supports_developer_role: Some(true),
            ..axum_provider::compat::Compat::default()
        });
        let context = Context {
            system: Some("be brief".into()),
            ..context()
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
        let body = adapter("openai-completions").request(
            &model(Api::OpenAiCompletions),
            &context,
            &Options::default(),
        );
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
}
