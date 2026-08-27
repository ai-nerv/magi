//! What a model is.

use crate::compat::Compat;
use axum_model::{Cost, ThinkingLevel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A wire protocol.
///
/// Not a vendor. This is the axis Pi got right and Tau did not: a provider is an identity and
/// an api is a protocol, and they are many-to-many. The surprises are the point — Fireworks,
/// GitHub Copilot, MiniMax and Vercel all speak Anthropic's Messages API, and OpenAI itself
/// speaks Responses rather than the Completions dialect twenty other vendors copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Api {
    /// Anthropic's Messages API.
    AnthropicMessages,
    /// OpenAI's Chat Completions, and the twenty-odd vendors that copy it.
    #[serde(rename = "openai-completions")]
    OpenAiCompletions,
    /// OpenAI's Responses API.
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// Azure's hosting of Responses, which differs in routing and auth.
    #[serde(rename = "azure-openai-responses")]
    AzureOpenAiResponses,
    /// The ChatGPT subscription backend Codex uses.
    #[serde(rename = "openai-codex-responses")]
    OpenAiCodexResponses,
    /// Google's Generative Language API.
    GoogleGenerativeAi,
    /// Google's Vertex hosting, which differs in auth and endpoint shape.
    GoogleVertex,
    /// Amazon Bedrock's Converse streaming API.
    BedrockConverseStream,
    /// Mistral's Conversations API.
    MistralConversations,
    /// Pi's own protocol, for a pi-compatible endpoint.
    PiMessages,
}

impl Api {
    /// The name used in configuration and errors.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAiCompletions => "openai-completions",
            Self::OpenAiResponses => "openai-responses",
            Self::AzureOpenAiResponses => "azure-openai-responses",
            Self::OpenAiCodexResponses => "openai-codex-responses",
            Self::GoogleGenerativeAi => "google-generative-ai",
            Self::GoogleVertex => "google-vertex",
            Self::BedrockConverseStream => "bedrock-converse-stream",
            Self::MistralConversations => "mistral-conversations",
            Self::PiMessages => "pi-messages",
        }
    }

    /// Every protocol axum knows.
    #[must_use]
    pub const fn all() -> [Self; 10] {
        [
            Self::AnthropicMessages,
            Self::OpenAiCompletions,
            Self::OpenAiResponses,
            Self::AzureOpenAiResponses,
            Self::OpenAiCodexResponses,
            Self::GoogleGenerativeAi,
            Self::GoogleVertex,
            Self::BedrockConverseStream,
            Self::MistralConversations,
            Self::PiMessages,
        ]
    }
}

/// What a model accepts as input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Text prompts.
    Text,
    /// Image parts in user messages.
    Image,
}

/// One model, as a provider offers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    /// Id sent on the wire.
    pub id: String,
    /// Name shown to a person.
    pub name: String,
    /// Which provider offers it.
    ///
    /// Filled in from the provider when a declaration omits it, which every declaration does:
    /// repeating the provider id on each of its own models is noise that can disagree.
    #[serde(default)]
    pub provider: String,
    /// Which protocol it speaks. Filled in from the provider for the same reason.
    #[serde(default = "default_api")]
    pub api: Api,
    /// Whether it can reason.
    #[serde(default)]
    pub reasoning: bool,
    /// What it accepts.
    #[serde(default = "default_input")]
    pub input: Vec<Modality>,
    /// Tokens it can hold.
    pub context_window: u64,
    /// Tokens it will produce in one turn.
    pub max_tokens: u64,
    /// Dollars per million tokens.
    #[serde(default)]
    pub cost: Cost,
    /// How this model names each thinking level, and which it cannot do.
    ///
    /// A missing key takes the provider default; an explicit `None` marks the level
    /// unsupported, which is different from unmapped and has to stay tellable apart.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub thinking: BTreeMap<ThinkingLevel, Option<String>>,
    /// Per-model overrides for its protocol's quirks.
    ///
    /// Absent means "whatever the base URL implies", which is what keeps a new provider to one
    /// table row. Present means someone found a case detection got wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<Compat>,
}

fn default_api() -> Api {
    Api::OpenAiCompletions
}

fn default_input() -> Vec<Modality> {
    vec![Modality::Text]
}

impl Model {
    /// Whether this model accepts images.
    #[must_use]
    pub fn accepts_images(&self) -> bool {
        self.input.contains(&Modality::Image)
    }

    /// A fully-qualified name, as a person would type it.
    #[must_use]
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            provider: "p".into(),
            api: Api::OpenAiCompletions,
            reasoning: false,
            input: vec![Modality::Text],
            context_window: 1000,
            max_tokens: 100,
            cost: Cost::default(),
            thinking: BTreeMap::new(),
            compat: None,
        }
    }

    #[test]
    fn apis_name_themselves_as_configuration_does() {
        assert_eq!(Api::AnthropicMessages.as_str(), "anthropic-messages");
        assert_eq!(Api::OpenAiCompletions.as_str(), "openai-completions");
    }

    #[test]
    fn every_api_deserializes_from_the_name_it_prints() {
        // The two spellings are written by hand in different places; a drift between them
        // would make a catalog file reject a name the same binary prints back at the user.
        for api in Api::all() {
            let quoted = format!("\"{}\"", api.as_str());
            let parsed: Api = serde_json::from_str(&quoted)
                .unwrap_or_else(|e| panic!("{} does not round-trip: {e}", api.as_str()));
            assert_eq!(parsed, api);
        }
    }

    #[test]
    fn a_text_only_model_does_not_accept_images() {
        assert!(!model().accepts_images());
    }

    #[test]
    fn an_image_model_says_so() {
        let mut m = model();
        m.input.push(Modality::Image);
        assert!(m.accepts_images());
    }

    #[test]
    fn a_qualified_name_is_provider_slash_id() {
        assert_eq!(model().qualified(), "p/m");
    }

    #[test]
    fn an_unsupported_thinking_level_is_distinct_from_an_unmapped_one() {
        let mut m = model();
        m.thinking.insert(ThinkingLevel::Max, None);
        assert_eq!(
            m.thinking.get(&ThinkingLevel::Max),
            Some(&None),
            "mapped to unsupported"
        );
        assert_eq!(
            m.thinking.get(&ThinkingLevel::Low),
            None,
            "not mapped at all"
        );
    }
}
