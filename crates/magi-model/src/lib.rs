//! The provider-neutral message model.
//!
//! One shape every provider maps into and out of. Pure data: no HTTP, no provider imports, no
//! runtime. The `signature` field on [`Content::Text`] and [`Content::Thinking`], and
//! `thought_signature` on [`Content::ToolCall`], carry whatever a provider needs to accept its own
//! reasoning back; without them, changing model mid-session corrupts reasoning continuity.

pub mod estimate;
pub mod noted;
pub mod overflow;
mod request_frame;
pub mod scratch;
mod usage;

pub use request_frame::RequestFrame;
pub use usage::{Cost, Usage};

use serde::{Deserialize, Serialize};

/// Provider-advertised modalities and native thinking levels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Vec<String>>,
}

/// One inline raster image, base64 encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub data: String,
    pub media_type: String,
}

/// Whether `images` fit one message: at most four, each a known raster type, at most 128 KiB
/// before encoding and valid base64.
///
/// # Errors
/// The first limit broken, said for the person.
pub fn validate_images(images: &[Image]) -> Result<(), String> {
    if images.len() > 4 {
        return Err("At most four image attachments per message.".into());
    }
    for image in images {
        if !matches!(
            image.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
        ) {
            return Err("Unsupported image media type.".into());
        }
        if image.data.is_empty() || image.data.len() > (128usize * 1024).div_ceil(3) * 4 {
            return Err("Image attachment is empty or exceeds the 128 KiB inline limit. Resize it with Casper.".into());
        }
        let data = image.data.trim_end_matches('=');
        if !image.data.len().is_multiple_of(4)
            || image.data.len() - data.len() > 2
            || !data
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        {
            return Err("Image attachment has invalid base64.".into());
        }
    }
    Ok(())
}

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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<Image>,
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
    fn images_are_at_most_four_known_rasters_of_bounded_valid_base64() {
        let image = |data: &str, media_type: &str| Image {
            data: data.into(),
            media_type: media_type.into(),
        };
        let png = image("iVBORw0K", "image/png");
        let many = |n| std::iter::repeat_n(png.clone(), n).collect::<Vec<_>>();
        assert_eq!(validate_images(&[]), Ok(()));
        assert_eq!(validate_images(&many(4)), Ok(()));
        assert!(validate_images(&many(5)).is_err());
        for (data, media_type) in [
            ("iVBORw0K", "image/svg+xml"),
            ("", "image/png"),
            ("iVBORw0", "image/png"),
            ("iVBO!w0K", "image/png"),
            ("iV======", "image/png"),
        ] {
            assert!(
                validate_images(&[image(data, media_type)]).is_err(),
                "{data:?} {media_type}"
            );
        }
        let largest = "A".repeat((128usize * 1024).div_ceil(3) * 4);
        assert_eq!(validate_images(&[image(&largest, "image/jpeg")]), Ok(()));
        let larger = format!("{largest}AAAA");
        assert!(validate_images(&[image(&larger, "image/jpeg")]).is_err());
    }

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
