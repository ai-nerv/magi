//! The quirks one adapter has to absorb to serve twenty vendors.
//!
//! Every field here exists because some provider that claims to speak OpenAI Chat Completions
//! does not, in one specific way. Pi carries the same struct and detects a default from the
//! base URL; a model overrides a field only when detection got it wrong.
//!
//! This is what makes a provider a table row instead of a module. Without it, each vendor's
//! deviation becomes its own adapter, and Tau's 66,310 lines for three backends is what that
//! costs.

use serde::{Deserialize, Serialize};

/// Which request field caps the response length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    /// The modern OpenAI name.
    MaxCompletionTokens,
    /// The original, still required by most copies.
    MaxTokens,
}

impl MaxTokensField {
    /// The JSON key to write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MaxCompletionTokens => "max_completion_tokens",
            Self::MaxTokens => "max_tokens",
        }
    }
}

/// How a provider wants reasoning requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingFormat {
    /// `reasoning_effort: "low"`, as OpenAI does it.
    #[serde(rename = "openai")]
    OpenAi,
    /// `reasoning: { effort }`.
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// `thinking: { type }`, plus `reasoning_effort` where supported.
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// `thinking: { type }`.
    Zai,
    /// Top-level `enable_thinking: bool`.
    Qwen,
}

/// Overrides for one model's protocol quirks.
///
/// Every field is optional: `None` means "take what the base URL implies". A `Compat` with
/// everything set is a provider detection cannot recognise, which is a bug report, not a
/// design.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compat {
    /// Whether `store` is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_store: Option<bool>,
    /// Whether the `developer` role is accepted in place of `system`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    /// Whether `reasoning_effort` is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_effort: Option<bool>,
    /// Whether `stream_options: {include_usage: true}` yields token counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_usage_in_streaming: Option<bool>,
    /// Whether streamed chunks carry `finish_reason`.
    ///
    /// When they do not, the stop reason is inferred from whether tool calls arrived — which
    /// is why a missing `finish_reason` is a compat flag and not a parse error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_finish_reason: Option<bool>,
    /// Which field caps the response length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<MaxTokensField>,
    /// Whether a tool result must repeat the tool's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_tool_result_name: Option<bool>,
    /// Whether thinking blocks must be flattened into text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_thinking_as_text: Option<bool>,
    /// How reasoning is requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<ThinkingFormat>,
}

/// A `Compat` with every question answered.
///
/// Serialisable because this, not the sparse `Compat`, is what a protocol description is
/// handed. A description that had to write `compat.thinking_format or "openai"` would be
/// keeping a second copy of the defaults, in another language, for the two of them to disagree
/// about later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Resolved {
    /// Whether `store` is accepted.
    pub supports_store: bool,
    /// Whether the `developer` role is accepted.
    pub supports_developer_role: bool,
    /// Whether `reasoning_effort` is accepted.
    pub supports_reasoning_effort: bool,
    /// Whether streaming reports usage.
    pub supports_usage_in_streaming: bool,
    /// Whether streaming reports a finish reason.
    pub supports_finish_reason: bool,
    /// Which field caps the response length.
    pub max_tokens_field: MaxTokensField,
    /// Whether a tool result must repeat the tool's name.
    pub requires_tool_result_name: bool,
    /// Whether thinking must be flattened into text.
    pub requires_thinking_as_text: bool,
    /// How reasoning is requested.
    pub thinking_format: ThinkingFormat,
}

impl Default for Resolved {
    /// What an OpenAI-compatible endpoint does unless it says otherwise.
    ///
    /// The conservative reading, not OpenAI's own: the `store` field, the `developer` role and
    /// the `max_completion_tokens` spelling are OpenAI extensions that most copies reject, so
    /// a provider opts into them rather than out. A wrong default that omits a field degrades;
    /// one that sends an unknown field is a 400.
    fn default() -> Self {
        Self {
            supports_store: false,
            supports_developer_role: false,
            supports_reasoning_effort: false,
            supports_usage_in_streaming: true,
            supports_finish_reason: true,
            max_tokens_field: MaxTokensField::MaxTokens,
            requires_tool_result_name: false,
            requires_thinking_as_text: false,
            thinking_format: ThinkingFormat::OpenAi,
        }
    }
}

impl Compat {
    /// Apply these overrides to the conservative defaults.
    ///
    /// There is no host sniffing here on purpose. Pi infers a dialect from the base URL, which
    /// means every vendor it supports is named in an adapter; magi keeps vendors in the
    /// catalog, so a provider states its own dialect and this crate stays free of vendor names.
    /// A proxy or a self-hosted endpoint declares what it actually speaks instead of being
    /// guessed at from a hostname that no longer resembles the vendor's.
    #[must_use]
    pub fn resolve(self) -> Resolved {
        let d = Resolved::default();
        Resolved {
            supports_store: self.supports_store.unwrap_or(d.supports_store),
            supports_developer_role: self
                .supports_developer_role
                .unwrap_or(d.supports_developer_role),
            supports_reasoning_effort: self
                .supports_reasoning_effort
                .unwrap_or(d.supports_reasoning_effort),
            supports_usage_in_streaming: self
                .supports_usage_in_streaming
                .unwrap_or(d.supports_usage_in_streaming),
            supports_finish_reason: self
                .supports_finish_reason
                .unwrap_or(d.supports_finish_reason),
            max_tokens_field: self.max_tokens_field.unwrap_or(d.max_tokens_field),
            requires_tool_result_name: self
                .requires_tool_result_name
                .unwrap_or(d.requires_tool_result_name),
            requires_thinking_as_text: self
                .requires_thinking_as_text
                .unwrap_or(d.requires_thinking_as_text),
            thinking_format: self.thinking_format.unwrap_or(d.thinking_format),
        }
    }
}

impl Compat {
    /// Layer these overrides on top of `base`, field by field.
    ///
    /// Per field, not per struct. A provider says what its whole endpoint does and a model
    /// states the one thing it does differently; taking the model's table wholesale would
    /// throw away everything the provider said, so a model that corrects one flag would
    /// silently un-correct the rest. That is a bug you find as a 400 from one model on a
    /// provider whose other models work.
    #[must_use]
    pub fn over(self, base: Option<Self>) -> Self {
        let Some(base) = base else { return self };
        Self {
            supports_store: self.supports_store.or(base.supports_store),
            supports_developer_role: self
                .supports_developer_role
                .or(base.supports_developer_role),
            supports_reasoning_effort: self
                .supports_reasoning_effort
                .or(base.supports_reasoning_effort),
            supports_usage_in_streaming: self
                .supports_usage_in_streaming
                .or(base.supports_usage_in_streaming),
            supports_finish_reason: self.supports_finish_reason.or(base.supports_finish_reason),
            max_tokens_field: self.max_tokens_field.or(base.max_tokens_field),
            requires_tool_result_name: self
                .requires_tool_result_name
                .or(base.requires_tool_result_name),
            requires_thinking_as_text: self
                .requires_thinking_as_text
                .or(base.requires_thinking_as_text),
            thinking_format: self.thinking_format.or(base.thinking_format),
        }
    }
}

/// The dialect a model speaks, taking its declaration or the conservative default.
#[must_use]
pub fn resolve(compat: Option<Compat>) -> Resolved {
    compat.unwrap_or_default().resolve()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_conservative_reading() {
        let d = Resolved::default();
        assert_eq!(d.max_tokens_field, MaxTokensField::MaxTokens);
        assert!(
            !d.supports_store,
            "an unknown field is a 400, a missing one degrades"
        );
        assert!(!d.supports_developer_role);
        assert_eq!(d.thinking_format, ThinkingFormat::OpenAi);
    }

    #[test]
    fn an_undeclared_model_gets_the_default() {
        assert_eq!(resolve(None), Resolved::default());
    }

    #[test]
    fn a_declaration_overrides_only_what_it_names() {
        let compat = Compat {
            max_tokens_field: Some(MaxTokensField::MaxCompletionTokens),
            supports_store: Some(true),
            ..Compat::default()
        };
        let r = resolve(Some(compat));
        assert_eq!(r.max_tokens_field, MaxTokensField::MaxCompletionTokens);
        assert!(r.supports_store);
        assert!(
            !r.supports_developer_role,
            "an unnamed field keeps its default"
        );
    }

    #[test]
    fn an_empty_declaration_changes_nothing() {
        assert_eq!(resolve(Some(Compat::default())), Resolved::default());
    }

    #[test]
    fn every_thinking_dialect_can_be_declared() {
        for format in [
            ThinkingFormat::OpenAi,
            ThinkingFormat::OpenRouter,
            ThinkingFormat::DeepSeek,
            ThinkingFormat::Zai,
            ThinkingFormat::Qwen,
        ] {
            let r = resolve(Some(Compat {
                thinking_format: Some(format),
                ..Compat::default()
            }));
            assert_eq!(r.thinking_format, format);
        }
    }

    #[test]
    fn max_tokens_fields_name_their_wire_keys() {
        assert_eq!(MaxTokensField::MaxTokens.as_str(), "max_tokens");
        assert_eq!(
            MaxTokensField::MaxCompletionTokens.as_str(),
            "max_completion_tokens"
        );
    }
}
