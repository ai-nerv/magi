//! One adapter per wire protocol.
//!
//! This is where vendor differences belong when they are too large for [`crate::compat`]: a
//! protocol has its own request shape and its own stream events, and no amount of flags makes
//! Anthropic's Messages API into OpenAI's Completions. Everything smaller than that is a
//! declaration in the catalog.
//!
//! Adapters are pure. `request` builds JSON, `on_event` folds one server-sent event into a
//! stream's state — no HTTP, no sockets, no clock. That is what lets every one of them be
//! tested against recorded bytes instead of a live account.

pub mod anthropic;

use crate::model::Model;
use axum_model::{Context, StopReason, ThinkingLevel, Usage};
use serde::{Deserialize, Serialize};

/// What a caller asks for beyond the conversation itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Options {
    /// How much reasoning to request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingLevel>,
    /// Cap the response, below the model's own maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// One thing that happened while a response streamed.
///
/// Deliberately smaller than the protocol's own event set: a caller wants to know that text
/// arrived, not that a content block of index 3 opened. Anything an adapter needs to remember
/// between events lives in [`StreamState`] instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    /// Response text.
    Text(String),
    /// Reasoning text.
    Thinking(String),
    /// Opaque provider state for the block being streamed, to be replayed verbatim.
    Signature(String),
    /// A tool call began.
    ToolCallStart {
        /// Provider-issued identity.
        id: String,
        /// Tool name.
        name: String,
    },
    /// Arguments for the tool call in progress, as raw JSON text.
    ToolCallArgs(String),
    /// The turn finished.
    Stop(StopReason),
    /// Token counts, which arrive at their own pace.
    Usage(Usage),
}

/// What an adapter carries between events of one response.
#[derive(Debug, Default)]
pub struct StreamState {
    /// The kind of block currently open, so a delta knows what it is extending.
    block: Option<Block>,
    /// Tokens seen so far.
    usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Block {
    Text,
    Thinking,
    ToolCall,
}

/// A wire protocol axum can speak.
pub trait Adapter: Send + Sync {
    /// The URL to post to.
    fn endpoint(&self, base_url: &str, model: &Model) -> String;

    /// Headers, given whatever credential was resolved.
    fn headers(&self, key: Option<&str>) -> Vec<(String, String)>;

    /// The request body.
    fn request(&self, model: &Model, context: &Context, options: &Options) -> serde_json::Value;

    /// Fold one server-sent event into the stream.
    ///
    /// Returns what the caller should be told. An event that only changes bookkeeping — a
    /// keep-alive, a block opening — yields nothing, which is why this returns a list rather
    /// than an `Option`: one event can produce several deltas or none.
    fn on_event(&self, state: &mut StreamState, event: &crate::sse::Event) -> Vec<Delta>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_to_asking_for_nothing_extra() {
        let options = Options::default();
        assert!(options.thinking.is_none());
        assert!(options.max_tokens.is_none());
    }

    #[test]
    fn a_fresh_stream_has_no_open_block() {
        assert!(StreamState::default().block.is_none());
    }
}
