//! The provider-neutral message model.
//!
//! One shape every provider maps into and out of. Pure data: no HTTP, no provider imports, no
//! runtime. `axon-provider` owns the wire, this crate owns what is on it.
//!
//! The thing that makes cross-provider switching survivable is the `signature` field on
//! [`Content::Text`] and [`Content::Thinking`], and `thought_signature` on
//! [`Content::ToolCall`]: opaque strings carrying whatever a provider needs to accept its own
//! reasoning back. Pi keeps three such slots and calls them signature carriers; without them,
//! changing model mid-session corrupts reasoning continuity.

mod usage;

pub use usage::{Cost, Usage};

use serde::{Deserialize, Serialize};

/// Why a turn stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished.
    EndTurn,
    /// The model asked for tools; the turn continues once they run.
    ToolUse,
    /// The output hit the token limit mid-generation.
    ///
    /// Every tool call in the turn must be failed: truncated JSON can still pass schema
    /// validation, so the arguments may be well-formed and wrong.
    Length,
    /// The user interrupted.
    Aborted,
    /// The turn failed.
    Error,
}

/// How much reasoning to ask for.
///
/// Provider-neutral levels; a model maps them to whatever it actually accepts, and marks the
/// ones it cannot do. Pi calls this a thinking level map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    /// No reasoning.
    Off,
    /// The smallest budget the model offers.
    Minimal,
    /// A small budget.
    Low,
    /// The usual budget.
    Medium,
    /// A large budget.
    High,
    /// The largest budget the model offers.
    Max,
}

/// One block of a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Content {
    /// Ordinary prose.
    Text {
        /// The text.
        text: String,
        /// Opaque provider state for this block, replayed verbatim.
        ///
        /// One of the three signature carriers. Never parsed, never generated — only stored
        /// and handed back, because only the provider that issued it can read it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Reasoning the model chose to expose.
    Thinking {
        /// The reasoning text.
        thinking: String,
        /// Opaque provider state for this block, replayed verbatim.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// An image, as base64 with its media type.
    Image {
        /// Base64 payload.
        data: String,
        /// IANA media type, e.g. `image/png`.
        media_type: String,
    },
    /// The model asking for a tool.
    ToolCall {
        /// Provider-issued identity, matched by [`Content::ToolResult`].
        id: String,
        /// Tool name.
        name: String,
        /// Arguments as JSON.
        arguments: serde_json::Value,
        /// Opaque provider state for this call, replayed verbatim.
        ///
        /// The third signature carrier. Google issues one per call rather than per message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    /// What a tool produced.
    ToolResult {
        /// The call this answers.
        id: String,
        /// Tool name. Some dialects require it on the result as well as the call.
        name: String,
        /// Text the model sees.
        content: String,
        /// Whether the tool failed.
        is_error: bool,
    },
}

/// Who produced a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The person.
    User,
    /// The model.
    Assistant,
    /// Tool output, which some dialects carry as its own role.
    Tool,
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who produced it.
    pub role: Role,
    /// Its blocks, in order.
    pub content: Vec<Content>,
    /// Why the turn stopped, on assistant messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// What it cost, on assistant messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Detail when `stop_reason` is [`StopReason::Error`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Message {
    /// A user message carrying one block of text.
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

    /// An assistant message carrying one block of text.
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

    /// All text blocks joined, which is what a print mode prints.
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

    /// The tool calls this message asks for.
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

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Name the model uses to call it.
    pub name: String,
    /// What it does, in the model's terms.
    pub description: String,
    /// JSON Schema for its arguments.
    pub parameters: serde_json::Value,
}

/// Everything a provider needs for one request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// Instructions that ride outside the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// Tools the model may call.
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
