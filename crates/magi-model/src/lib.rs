//! The provider-neutral message model.
//!
//! One shape every provider maps into and out of. Pure data: no HTTP, no provider imports, no
//! runtime. The `signature` field on [`Content::Text`] and [`Content::Thinking`], and
//! `thought_signature` on [`Content::ToolCall`], carry whatever a provider needs to accept its own
//! reasoning back; without them, changing model mid-session corrupts reasoning continuity.

pub mod noted;
pub mod scratch;
mod usage;

pub use usage::{Cost, Usage};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    /// The output hit the token limit mid-generation. Every tool call in the turn must be failed:
    /// truncated JSON can still pass schema validation.
    Length,
    Aborted,
    Error,
}

/// How much reasoning to ask for. Provider-neutral levels; a model maps them to what it accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Max,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Content {
    Text {
        text: String,
        /// Opaque provider state for this block, replayed verbatim. Never parsed, never generated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Image {
        data: String,
        /// IANA media type, e.g. `image/png`.
        media_type: String,
    },
    ToolCall {
        /// Provider-issued identity, matched by [`Content::ToolResult`].
        id: String,
        name: String,
        arguments: serde_json::Value,
        /// Opaque provider state for this call, replayed verbatim. Google issues one per call
        /// rather than per message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    ToolResult {
        id: String,
        /// Tool name. Some dialects require it on the result as well as the call.
        name: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    /// Tool output, which some dialects carry as its own role.
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Content>,
    /// Why the turn stopped, on assistant messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Message {
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![Content::Text {
                text: text.into(),
                signature: None,
            }],
            stop_reason: None,
            usage: None,
            error: None,
        }
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![Content::Text {
                text: text.into(),
                signature: None,
            }],
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
            error: None,
        }
    }

    #[must_use]
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = (&str, &str, &serde_json::Value)> {
        self.content.iter().filter_map(|c| match c {
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } => Some((id.as_str(), name.as_str(), arguments)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Everything a provider needs for one request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// Instructions that ride outside the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_message_carries_its_text() {
        assert_eq!(Message::user("hello").text(), "hello");
    }

    #[test]
    fn text_joins_only_text_blocks() {
        let message = Message {
            role: Role::Assistant,
            content: vec![
                Content::Thinking {
                    thinking: "hidden".into(),
                    signature: None,
                },
                Content::Text {
                    text: "shown".into(),
                    signature: None,
                },
            ],
            stop_reason: None,
            usage: None,
            error: None,
        };
        assert_eq!(message.text(), "shown");
    }

    #[test]
    fn tool_calls_are_enumerable() {
        let message = Message {
            role: Role::Assistant,
            content: vec![Content::ToolCall {
                id: "t1".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "a.rs"}),
                thought_signature: None,
            }],
            stop_reason: Some(StopReason::ToolUse),
            usage: None,
            error: None,
        };
        let calls: Vec<_> = message.tool_calls().collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "read");
    }

    #[test]
    fn a_signature_survives_a_round_trip() {
        let block = Content::Thinking {
            thinking: "reasoned".into(),
            signature: Some("opaque-provider-state".into()),
        };
        let json = serde_json::to_string(&block).expect("encode");
        assert_eq!(
            serde_json::from_str::<Content>(&json).expect("decode"),
            block
        );
    }

    #[test]
    fn an_absent_signature_is_not_serialized() {
        let block = Content::Text {
            text: "plain".into(),
            signature: None,
        };
        let json = serde_json::to_string(&block).expect("encode");
        assert!(!json.contains("signature"), "{json}");
    }

    #[test]
    fn thinking_levels_order_from_off_to_max() {
        assert!(ThinkingLevel::Off < ThinkingLevel::Low);
        assert!(ThinkingLevel::High < ThinkingLevel::Max);
    }
}

/// One thing that happened while an answer streamed. Provider-neutral: melchior speaks the
/// protocols and hands back a `Said`, which the broker turns into these.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    Thinking(String),
    /// Opaque provider state for the block being streamed, to be replayed verbatim.
    Signature(String),
    ToolCallStart {
        id: String,
        name: String,
    },
    /// Arguments for the tool call in progress, as raw JSON text.
    ToolCallArgs(String),
    Stop(StopReason),
    /// Token counts, which arrive at their own pace.
    Usage(Usage),
}
