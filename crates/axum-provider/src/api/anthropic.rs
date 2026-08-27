//! Anthropic's Messages API.

use super::{Adapter, Block, Delta, Options, StreamState};
use crate::model::Model;
use crate::sse;
use axum_model::{Content, Context, Message, Role, StopReason, ThinkingLevel, Usage};
use serde_json::{Map, Value, json};

/// The API version header. Anthropic dates its protocol rather than numbering it.
const VERSION: &str = "2023-06-01";

/// Reasoning budgets, in tokens, for each level a caller can ask for.
///
/// A model may override these in its catalog entry; these are what a model that says nothing
/// gets. `Off` is absent rather than zero: the request omits thinking entirely, which is a
/// different thing from asking for none of it.
const fn budget(level: ThinkingLevel) -> Option<u64> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some(1024),
        ThinkingLevel::Low => Some(4096),
        ThinkingLevel::Medium => Some(16384),
        ThinkingLevel::High => Some(32768),
        ThinkingLevel::Max => Some(63999),
    }
}

/// The Messages adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Anthropic;

impl Adapter for Anthropic {
    fn endpoint(&self, base_url: &str, _model: &Model) -> String {
        format!("{}/v1/messages", base_url.trim_end_matches('/'))
    }

    fn headers(&self, key: Option<&str>) -> Vec<(String, String)> {
        let mut out = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("anthropic-version".to_owned(), VERSION.to_owned()),
        ];
        if let Some(key) = key {
            out.push(("x-api-key".to_owned(), key.to_owned()));
        }
        out
    }

    fn request(&self, model: &Model, context: &Context, options: &Options) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(model.id));
        body.insert("stream".into(), json!(true));
        body.insert(
            "max_tokens".into(),
            json!(
                options
                    .max_tokens
                    .unwrap_or(model.max_tokens)
                    .min(model.max_tokens)
            ),
        );
        if let Some(system) = &context.system {
            body.insert("system".into(), json!(system));
        }
        body.insert("messages".into(), json!(messages(&context.messages)));
        if !context.tools.is_empty() {
            let tools: Vec<Value> = context
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body.insert("tools".into(), json!(tools));
        }
        // Thinking is requested by budget, and a budget must leave room for a response: asking
        // for the whole of `max_tokens` as reasoning yields a turn with nothing in it.
        if let Some(level) = options.thinking {
            if let Some(tokens) = budget(level) {
                let cap = model.max_tokens.saturating_sub(1024).max(1024);
                body.insert(
                    "thinking".into(),
                    json!({ "type": "enabled", "budget_tokens": tokens.min(cap) }),
                );
            }
        }
        Value::Object(body)
    }

    fn on_event(&self, state: &mut StreamState, event: &sse::Event) -> Vec<Delta> {
        let Ok(data): Result<Value, _> = serde_json::from_str(&event.data) else {
            return Vec::new();
        };
        match event.name.as_str() {
            "message_start" => {
                state.usage = usage_of(&data["message"]["usage"]);
                vec![Delta::Usage(state.usage)]
            }
            "content_block_start" => {
                let block = &data["content_block"];
                match block["type"].as_str() {
                    Some("thinking") => {
                        state.block = Some(Block::Thinking);
                        Vec::new()
                    }
                    Some("tool_use") => {
                        state.block = Some(Block::ToolCall);
                        vec![Delta::ToolCallStart {
                            id: block["id"].as_str().unwrap_or_default().to_owned(),
                            name: block["name"].as_str().unwrap_or_default().to_owned(),
                        }]
                    }
                    _ => {
                        state.block = Some(Block::Text);
                        Vec::new()
                    }
                }
            }
            "content_block_delta" => {
                let delta = &data["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => text_of(delta, "text")
                        .map(Delta::Text)
                        .into_iter()
                        .collect(),
                    Some("thinking_delta") => text_of(delta, "thinking")
                        .map(Delta::Thinking)
                        .into_iter()
                        .collect(),
                    // The signature arrives at the end of a thinking block and must be replayed
                    // verbatim for the model to accept its own reasoning back.
                    Some("signature_delta") => text_of(delta, "signature")
                        .map(Delta::Signature)
                        .into_iter()
                        .collect(),
                    Some("input_json_delta") => text_of(delta, "partial_json")
                        .map(Delta::ToolCallArgs)
                        .into_iter()
                        .collect(),
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                state.block = None;
                Vec::new()
            }
            "message_delta" => {
                let mut out = Vec::new();
                // Output tokens are only final here, so the running total is replaced rather
                // than added to: adding would double-count what `message_start` reported.
                if let Some(output) = data["usage"]["output_tokens"].as_u64() {
                    state.usage.output = output;
                    out.push(Delta::Usage(state.usage));
                }
                if let Some(reason) = data["delta"]["stop_reason"].as_str() {
                    out.push(Delta::Stop(stop_reason(reason)));
                }
                out
            }
            // A ping keeps the connection warm and carries nothing; an error is surfaced by the
            // transport, which sees the status the stream arrived with.
            _ => Vec::new(),
        }
    }
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn usage_of(value: &Value) -> Usage {
    Usage {
        input: value["input_tokens"].as_u64().unwrap_or(0),
        output: value["output_tokens"].as_u64().unwrap_or(0),
        cache_read: value["cache_read_input_tokens"].as_u64().unwrap_or(0),
        cache_write: value["cache_creation_input_tokens"].as_u64().unwrap_or(0),
    }
}

/// Anthropic's stop reasons, in axum's vocabulary.
///
/// `max_tokens` becomes [`StopReason::Length`], which the turn loop treats as poison for every
/// tool call in the turn: truncated JSON can still pass schema validation.
fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::Length,
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        _ => StopReason::EndTurn,
    }
}

/// Turn the neutral conversation into Anthropic's message list.
///
/// Tool results are user-role blocks here rather than a role of their own, which is the single
/// largest shape difference from the Completions dialect.
fn messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for message in messages {
        let role = match message.role {
            Role::Assistant => "assistant",
            Role::User | Role::Tool => "user",
        };
        let blocks: Vec<Value> = message.content.iter().filter_map(block).collect();
        if blocks.is_empty() {
            continue;
        }
        // Consecutive same-role messages are merged: the API rejects two user turns in a row,
        // and a tool result following a user message is exactly that shape.
        match out.last_mut() {
            Some(last) if last["role"] == role => {
                if let Some(content) = last["content"].as_array_mut() {
                    content.extend(blocks);
                }
            }
            _ => out.push(json!({ "role": role, "content": blocks })),
        }
    }
    out
}

fn block(content: &Content) -> Option<Value> {
    Some(match content {
        Content::Text { text, .. } if text.is_empty() => return None,
        Content::Text { text, .. } => json!({ "type": "text", "text": text }),
        Content::Thinking {
            thinking,
            signature,
        } => {
            // A thinking block without its signature is refused by the API, so one that lost it
            // is dropped rather than sent to be rejected.
            let signature = signature.as_ref()?;
            json!({ "type": "thinking", "thinking": thinking, "signature": signature })
        }
        Content::Image { data, media_type } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        }),
        Content::ToolCall {
            id,
            name,
            arguments,
            ..
        } => json!({ "type": "tool_use", "id": id, "name": name, "input": arguments }),
        Content::ToolResult {
            id,
            content,
            is_error,
            ..
        } => json!({
            "type": "tool_result",
            "tool_use_id": id,
            "content": content,
            "is_error": is_error,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Api, Modality};
    use axum_model::{Cost, Tool};
    use std::collections::BTreeMap;

    fn model() -> Model {
        Model {
            id: "claude-sonnet-4-5".into(),
            name: "Claude Sonnet 4.5".into(),
            provider: "anthropic".into(),
            api: Api::AnthropicMessages,
            reasoning: true,
            input: vec![Modality::Text, Modality::Image],
            context_window: 200_000,
            max_tokens: 64_000,
            cost: Cost::default(),
            thinking: BTreeMap::new(),
            compat: None,
        }
    }

    fn request(context: Context, options: Options) -> Value {
        Anthropic.request(&model(), &context, &options)
    }

    /// Fold a recorded stream and collect what a caller would be told.
    fn stream(events: &[(&str, &str)]) -> Vec<Delta> {
        let mut state = StreamState::default();
        events
            .iter()
            .flat_map(|(name, data)| {
                Anthropic.on_event(
                    &mut state,
                    &sse::Event {
                        name: (*name).to_owned(),
                        data: (*data).to_owned(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn the_endpoint_is_built_from_the_base_url() {
        assert_eq!(
            Anthropic.endpoint("https://api.anthropic.com", &model()),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        assert_eq!(
            Anthropic.endpoint("https://api.anthropic.com/", &model()),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn headers_carry_the_protocol_date_and_the_key() {
        let headers = Anthropic.headers(Some("secret"));
        assert!(headers.contains(&("anthropic-version".into(), VERSION.into())));
        assert!(headers.contains(&("x-api-key".into(), "secret".into())));
    }

    #[test]
    fn a_missing_key_omits_the_header_rather_than_sending_an_empty_one() {
        let headers = Anthropic.headers(None);
        assert!(!headers.iter().any(|(name, _)| name == "x-api-key"));
    }

    #[test]
    fn a_request_streams_and_names_its_model() {
        let body = request(
            Context {
                messages: vec![Message::user("hi")],
                ..Context::default()
            },
            Options::default(),
        );
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn max_tokens_cannot_exceed_what_the_model_will_produce() {
        let body = request(
            Context {
                messages: vec![Message::user("hi")],
                ..Context::default()
            },
            Options {
                max_tokens: Some(999_999),
                ..Options::default()
            },
        );
        assert_eq!(body["max_tokens"], 64_000);
    }

    #[test]
    fn thinking_off_omits_the_field_entirely() {
        let body = request(
            Context {
                messages: vec![Message::user("hi")],
                ..Context::default()
            },
            Options {
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
    fn a_thinking_budget_leaves_room_for_a_response() {
        let mut small = model();
        small.max_tokens = 2048;
        let body = Anthropic.request(
            &small,
            &Context {
                messages: vec![Message::user("hi")],
                ..Context::default()
            },
            &Options {
                thinking: Some(ThinkingLevel::Max),
                ..Options::default()
            },
        );
        let budget = body["thinking"]["budget_tokens"]
            .as_u64()
            .expect("a budget");
        assert!(
            budget < small.max_tokens,
            "asking for all of it yields an empty turn"
        );
    }

    #[test]
    fn tools_carry_their_schema() {
        let body = request(
            Context {
                messages: vec![Message::user("hi")],
                tools: vec![Tool {
                    name: "read".into(),
                    description: "read a file".into(),
                    parameters: json!({ "type": "object" }),
                }],
                ..Context::default()
            },
            Options::default(),
        );
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn a_tool_result_is_a_user_block_not_a_role() {
        let body = request(
            Context {
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
            },
            Options::default(),
        );
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["type"], "tool_result");
    }

    #[test]
    fn consecutive_same_role_messages_are_merged() {
        // The API refuses two user turns in a row, and a tool result after a user message is
        // exactly that shape.
        let body = request(
            Context {
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
            },
            Options::default(),
        );
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1, "merged into one user turn");
        assert_eq!(messages[0]["content"].as_array().expect("blocks").len(), 2);
    }

    #[test]
    fn a_thinking_block_without_its_signature_is_dropped() {
        // The API refuses one, so sending it buys a 400 instead of a turn.
        let body = request(
            Context {
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
            },
            Options::default(),
        );
        let blocks = body["messages"][0]["content"].as_array().expect("blocks");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["text"], "kept");
    }

    #[test]
    fn a_signed_thinking_block_is_replayed_verbatim() {
        let body = request(
            Context {
                messages: vec![Message {
                    role: Role::Assistant,
                    content: vec![Content::Thinking {
                        thinking: "reasoned".into(),
                        signature: Some("opaque".into()),
                    }],
                    stop_reason: None,
                    usage: None,
                    error: None,
                }],
                ..Context::default()
            },
            Options::default(),
        );
        assert_eq!(body["messages"][0]["content"][0]["signature"], "opaque");
    }

    #[test]
    fn text_deltas_stream() {
        let deltas = stream(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
            ),
        ]);
        assert_eq!(
            deltas,
            vec![Delta::Text("Hel".into()), Delta::Text("lo".into())]
        );
    }

    #[test]
    fn a_thinking_block_yields_its_text_and_its_signature() {
        let deltas = stream(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"thinking"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"why"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
            ),
        ]);
        assert_eq!(
            deltas,
            vec![
                Delta::Thinking("why".into()),
                Delta::Signature("sig".into())
            ]
        );
    }

    #[test]
    fn a_tool_call_announces_itself_then_streams_its_arguments() {
        let deltas = stream(&[
            (
                "content_block_start",
                r#"{"index":0,"content_block":{"type":"tool_use","id":"t1","name":"read"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"th\":\"a\"}"}}"#,
            ),
        ]);
        assert_eq!(
            deltas,
            vec![
                Delta::ToolCallStart {
                    id: "t1".into(),
                    name: "read".into()
                },
                Delta::ToolCallArgs("{\"pa".into()),
                Delta::ToolCallArgs("th\":\"a\"}".into()),
            ]
        );
    }

    #[test]
    fn usage_arrives_at_the_start_and_is_completed_at_the_end() {
        let deltas = stream(&[
            (
                "message_start",
                r#"{"message":{"usage":{"input_tokens":10,"cache_read_input_tokens":90}}}"#,
            ),
            (
                "message_delta",
                r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            ),
        ]);
        let Delta::Usage(first) = deltas[0] else {
            panic!("expected usage first, got {:?}", deltas[0]);
        };
        assert_eq!(first.input, 10);
        assert_eq!(first.cache_read, 90);

        let Delta::Usage(last) = deltas[1] else {
            panic!("expected usage again, got {:?}", deltas[1]);
        };
        assert_eq!(last.output, 5);
        assert_eq!(last.input, 10, "the earlier counts survive");
        assert_eq!(deltas[2], Delta::Stop(StopReason::EndTurn));
    }

    #[test]
    fn stop_reasons_map_to_axums_vocabulary() {
        assert_eq!(stop_reason("tool_use"), StopReason::ToolUse);
        assert_eq!(stop_reason("max_tokens"), StopReason::Length);
        assert_eq!(stop_reason("end_turn"), StopReason::EndTurn);
        assert_eq!(stop_reason("stop_sequence"), StopReason::EndTurn);
    }

    #[test]
    fn a_keep_alive_carries_nothing() {
        assert!(stream(&[("ping", "{}")]).is_empty());
    }

    #[test]
    fn a_malformed_event_is_ignored_rather_than_fatal() {
        // A stream is a live connection; one unparseable frame must not lose the turn.
        assert!(stream(&[("content_block_delta", "not json")]).is_empty());
    }

    #[test]
    fn an_unknown_event_is_ignored() {
        assert!(stream(&[("something_new", "{}")]).is_empty());
    }
}
