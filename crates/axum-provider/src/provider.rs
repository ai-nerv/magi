//! A provider: an identity, a base URL, a credential, and a catalog.
//!
//! Deliberately thin. Pi's `groq.ts` is fifteen lines â id, name, base URL, env var, models,
//! api â and ours is a struct literal of the same six fields. That thinness is the whole
//! argument for keeping providers in-process: at fifteen lines each, forty of them cost less
//! than one process boundary.

use crate::model::{Api, Model};
use serde::{Deserialize, Serialize};

/// Where a credential comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Auth {
    /// An API key read from the first environment variable that is set.
    ///
    /// Several names because vendors rename them and users have old ones exported; taking the
    /// first that is set is kinder than demanding the current spelling.
    ApiKey {
        /// Variables to try, in order.
        vars: Vec<String>,
    },
    /// An OAuth flow against a subscription account.
    ///
    /// Distinct from an API key because there is nothing a person can export to satisfy it:
    /// the answer to "how do I enable this" is a command, not a variable.
    #[serde(rename = "oauth")]
    OAuth {
        /// What a person signs in to, for the message that tells them to.
        service: String,
        /// Where the browser is sent.
        ///
        /// Optional, with the rest of them, because a provider can be in the catalog before
        /// this build knows how to sign in to it: the endpoints and the client id are things
        /// a vendor issues, and inventing plausible ones would turn "not supported yet" into
        /// a sign-in that fails for a reason nobody can act on. Absent, `axum auth login`
        /// says exactly what is missing.
        #[serde(default)]
        authorize_url: Option<String>,
        /// Where a code, or a refresh token, is exchanged.
        #[serde(default)]
        token_url: Option<String>,
        /// The public client this build identifies as.
        #[serde(default)]
        client_id: Option<String>,
        /// What is asked for.
        #[serde(default)]
        scopes: Vec<String>,
    },
    /// AWS SigV4, from the usual credential chain.
    AwsSigV4,
    /// Google Application Default Credentials.
    GoogleAdc,
    /// No credential; a local endpoint.
    None,
}

impl Auth {
    /// What a person would have to do to enable this.
    #[must_use]
    pub fn requirement(&self) -> String {
        match self {
            Self::ApiKey { vars } => format!("set {}", vars.join(" or ")),
            Self::OAuth { service, .. } => format!("sign in to {service}"),
            // What is true, not what would be true if these were implemented. `resolve`
            // returns `None` for both whatever the machine holds, so a person who followed
            // "configure AWS credentials" would find nothing had changed and go looking for
            // the mistake in their own setup.
            Self::AwsSigV4 => "not yet: signing requests with AWS credentials".to_owned(),
            Self::GoogleAdc => "not yet: reading Google application default credentials".to_owned(),
            Self::None => String::new(),
        }
    }

    /// An API key from any of these variables.
    #[must_use]
    pub fn env(vars: &[&str]) -> Self {
        Self::ApiKey {
            vars: vars.iter().map(|v| (*v).to_owned()).collect(),
        }
    }

    /// Read the credential, if one is set.
    #[must_use]
    pub fn resolve(&self) -> Option<String> {
        match self {
            Self::ApiKey { vars } => vars
                .iter()
                .filter_map(|v| std::env::var(v).ok())
                .find(|value| !value.trim().is_empty()),
            // Whatever was stored, fresh or not. `is_configured` asks whether a person has
            // signed in, which a stale token still answers yes to; renewing it is the job of
            // `bearer`, which can wait on a network round trip and this cannot.
            Self::OAuth { .. } => None,
            // A credential chain is a signing implementation, not a lookup: SigV4 signs each
            // request and ADC mints a short-lived token. Neither is written, and `requirement`
            // says so rather than implying a variable would do it.
            Self::AwsSigV4 | Self::GoogleAdc | Self::None => None,
        }
    }

    /// The variables a person would have to set.
    #[must_use]
    pub fn vars(&self) -> &[String] {
        match self {
            Self::ApiKey { vars } => vars,
            Self::OAuth { .. } | Self::AwsSigV4 | Self::GoogleAdc | Self::None => &[],
        }
    }
}

/// One vendor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "ProviderDecl")]
pub struct Provider {
    /// Id used in configuration and in a qualified model name.
    pub id: String,
    /// Name shown to a person.
    pub name: String,
    /// Where its API lives, when that is fixed.
    ///
    /// `None` for the endpoints derived from configuration — a Bedrock region, an Azure
    /// resource, a Vertex project, a Cloudflare account. Detection has nothing to read until
    /// those are set, so such a provider carries an explicit `compat` instead of relying on it.
    pub base_url: Option<String>,
    /// Which protocol it speaks.
    pub api: Api,
    /// How to authenticate.
    pub auth: Auth,
    /// Protocol overrides applied to every model this provider offers.
    ///
    /// Where a provider has no fixed `base_url`, detection has no host to read and this is
    /// the only way its dialect can be stated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<crate::compat::Compat>,
    /// What it offers.
    pub models: Vec<Model>,
}

impl Provider {
    /// Whether a credential for this provider is present.
    ///
    /// For an OAuth provider that means "has this person signed in", which a token that has
    /// since gone stale still answers yes to: it is renewable, and renewing is what happens on
    /// the way to the next request. Answering no would list a provider as unavailable for the
    /// hour between an expiry and its use.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        match &self.auth {
            Auth::None => true,
            Auth::OAuth { .. } => {
                crate::oauth::Store::load().is_ok_and(|store| store.get(&self.id).is_some())
            }
            _ => self.auth.resolve().is_some(),
        }
    }

    /// One of its models by id.
    #[must_use]
    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }
}

/// A provider exactly as a catalog file declares it.
///
/// Separate from [`Provider`] so the stamping below happens once, on the way in, rather than
/// being a rule every reader has to remember: a model always knows its own provider and api.
#[derive(Debug, Clone, PartialEq, Deserialize)]
struct ProviderDecl {
    id: String,
    name: String,
    #[serde(default)]
    base_url: Option<String>,
    api: Api,
    auth: Auth,
    #[serde(default)]
    compat: Option<crate::compat::Compat>,
    #[serde(default)]
    models: Vec<Model>,
}

impl From<ProviderDecl> for Provider {
    fn from(decl: ProviderDecl) -> Self {
        let ProviderDecl {
            id,
            name,
            base_url,
            api,
            auth,
            compat,
            mut models,
        } = decl;
        for model in &mut models {
            model.provider.clone_from(&id);
            model.api = api;
            // A provider-level override is the floor a model builds on, field by field. It
            // was `.or()` -- take the model's whole table if it has one -- which meant a
            // model correcting one flag threw away every other thing the provider had said.
            model.compat = Some(model.compat.unwrap_or_default().over(compat));
        }
        Self {
            id,
            name,
            base_url,
            api,
            auth,
            compat,
            models,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider as a config would declare it.
    ///
    /// JSON rather than a config file: this crate holds the schema, and the thing that turns a
    /// Lua table into one of these lives in the binary. Testing the schema through a file
    /// format would tie it to a loader it does not depend on.
    fn declared(json: serde_json::Value) -> Provider {
        serde_json::from_value(json).expect("a provider")
    }

    fn groq() -> serde_json::Value {
        serde_json::json!({
            "id": "groq",
            "name": "Groq",
            "api": "openai-completions",
            "base_url": "https://api.groq.com/openai/v1",
            "auth": { "kind": "api-key", "vars": ["GROQ_API_KEY"] },
            "models": [
                { "id": "a", "name": "A", "context_window": 131072, "max_tokens": 8192 },
                { "id": "b", "name": "B", "context_window": 131072, "max_tokens": 8192,
                  "reasoning": true },
            ]
        })
    }

    #[test]
    fn a_declaration_stamps_the_provider_and_api_onto_every_model() {
        let p = declared(groq());
        assert_eq!(p.models.len(), 2);
        assert!(p.models.iter().all(|m| m.provider == "groq"));
        assert!(p.models.iter().all(|m| m.api == Api::OpenAiCompletions));
    }

    #[test]
    fn omitted_model_fields_take_their_defaults() {
        let p = declared(groq());
        let a = p.model("a").expect("model a");
        assert!(!a.reasoning, "reasoning defaults off");
        assert_eq!(a.input, vec![crate::model::Modality::Text]);
        assert_eq!(a.cost.input, 0.0);
    }

    #[test]
    fn a_provider_with_no_auth_is_always_configured() {
        let p = declared(serde_json::json!({
            "id": "local", "name": "Local", "api": "openai-completions",
            "base_url": "http://localhost:11434/v1",
            "auth": { "kind": "none" },
            "models": [{ "id": "m", "name": "M", "context_window": 8192, "max_tokens": 4096 }]
        }));
        assert!(p.is_configured());
        assert!(p.auth.requirement().is_empty());
    }

    #[test]
    fn a_provider_without_its_variable_set_is_not_configured() {
        let p = declared(serde_json::json!({
            "id": "x", "name": "X", "api": "openai-completions",
            "base_url": "https://example.com/v1",
            "auth": { "kind": "api-key", "vars": ["AXUM_TEST_KEY_DEFINITELY_UNSET"] },
            "models": [{ "id": "m", "name": "M", "context_window": 8192, "max_tokens": 4096 }]
        }));
        assert!(!p.is_configured());
        assert_eq!(p.auth.requirement(), "set AXUM_TEST_KEY_DEFINITELY_UNSET");
    }

    #[test]
    fn a_credential_that_is_not_a_variable_says_what_to_do_instead() {
        let oauth = Auth::OAuth {
            service: "ChatGPT".into(),
            authorize_url: Some("https://example.test/authorize".into()),
            token_url: Some("https://example.test/token".into()),
            client_id: Some("c".into()),
            scopes: Vec::new(),
        };
        assert_eq!(oauth.requirement(), "sign in to ChatGPT");
        assert!(oauth.vars().is_empty(), "there is nothing to export");
        // "Not yet", not "configure them": `resolve` answers `None` whatever the machine
        // holds, so telling somebody to configure credentials sends them to fix a setup that
        // was never the problem.
        assert!(Auth::AwsSigV4.requirement().starts_with("not yet"));
        assert!(Auth::AwsSigV4.resolve().is_none());
        assert!(Auth::GoogleAdc.requirement().starts_with("not yet"));
    }

    #[test]
    fn a_provider_level_compat_reaches_every_model() {
        let p = declared(serde_json::json!({
            "id": "cf", "name": "Cloudflare", "api": "openai-completions",
            "auth": { "kind": "none" },
            "compat": { "supports_finish_reason": false },
            "models": [{ "id": "m", "name": "M", "context_window": 8192, "max_tokens": 4096 }]
        }));
        let compat = p.model("m").expect("model").compat.expect("compat");
        assert_eq!(compat.supports_finish_reason, Some(false));
    }

    #[test]
    fn a_model_may_override_its_providers_compat() {
        let p = declared(serde_json::json!({
            "id": "x", "name": "X", "api": "openai-completions",
            "auth": { "kind": "none" },
            "compat": { "supports_finish_reason": false },
            "models": [{ "id": "m", "name": "M", "context_window": 8192, "max_tokens": 4096,
                         "compat": { "supports_store": true } }]
        }));
        let compat = p.model("m").expect("model").compat.expect("compat");
        assert_eq!(compat.supports_store, Some(true), "the model's own wins");
    }

    #[test]
    fn a_provider_may_have_no_fixed_base_url() {
        let p = declared(serde_json::json!({
            "id": "bedrock", "name": "Bedrock", "api": "bedrock-converse-stream",
            "auth": { "kind": "aws-sig-v4" },
            "models": [{ "id": "m", "name": "M", "context_window": 8192, "max_tokens": 4096 }]
        }));
        assert!(
            p.base_url.is_none(),
            "the endpoint comes from configuration"
        );
    }

    #[test]
    fn an_unknown_model_is_absent_rather_than_a_panic() {
        assert!(declared(groq()).model("nope").is_none());
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    /// A provider stating its dialect, and one model correcting one flag.
    fn provider() -> Provider {
        serde_json::from_value(serde_json::json!({
            "id": "p", "name": "P", "api": "openai-completions", "auth": { "kind": "none" },
            "compat": {
                "supports_finish_reason": false,
                "max_tokens_field": "max_completion_tokens",
                "requires_tool_result_name": true
            },
            "models": [
                { "id": "plain", "name": "Plain", "context_window": 1000, "max_tokens": 100 },
                { "id": "odd", "name": "Odd", "context_window": 1000, "max_tokens": 100,
                  "compat": { "supports_store": true } }
            ]
        }))
        .expect("provider")
    }

    #[test]
    fn a_model_correcting_one_flag_keeps_the_rest_of_its_providers() {
        // This was `.or()`: the model's whole table replaced the provider's, so declaring one
        // exception silently un-declared everything else. It presents as a 400 from one model
        // on a provider whose other models work, which is a long way from the cause.
        let compat = provider()
            .model("odd")
            .expect("model")
            .compat
            .expect("compat");
        assert_eq!(compat.supports_store, Some(true), "the model's own wins");
        assert_eq!(
            compat.supports_finish_reason,
            Some(false),
            "the provider's survives"
        );
        assert_eq!(
            compat.max_tokens_field,
            Some(crate::compat::MaxTokensField::MaxCompletionTokens),
            "and so does this one"
        );
        assert_eq!(compat.requires_tool_result_name, Some(true));
    }

    #[test]
    fn a_model_stating_nothing_gets_all_of_its_providers() {
        let compat = provider()
            .model("plain")
            .expect("model")
            .compat
            .expect("compat");
        assert_eq!(compat.supports_finish_reason, Some(false));
        assert_eq!(compat.requires_tool_result_name, Some(true));
    }

    #[test]
    fn a_model_may_still_contradict_its_provider() {
        // Overriding must remain possible; the fix is about the fields nobody mentioned.
        let p: Provider = serde_json::from_value(serde_json::json!({
            "id": "p", "name": "P", "api": "openai-completions", "auth": { "kind": "none" },
            "compat": { "supports_finish_reason": false },
            "models": [{ "id": "m", "name": "M", "context_window": 1000, "max_tokens": 100,
                         "compat": { "supports_finish_reason": true } }]
        }))
        .expect("provider");
        let compat = p.model("m").expect("model").compat.expect("compat");
        assert_eq!(compat.supports_finish_reason, Some(true));
    }
}
