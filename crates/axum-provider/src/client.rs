//! Streaming a turn from a provider.
//!
//! The only part of this crate that does I/O. Everything it needs to know about a protocol
//! comes from an [`Adapter`]; everything it needs to know about a vendor comes from the
//! catalog. It knows neither.

use crate::api::{Adapter, Delta, Options};
use crate::model::{Api, Model};
use crate::provider::Provider;
use crate::retry::RetryClass;
use crate::sse;
use axum_model::Context;
use futures_util::StreamExt;

/// Why a turn could not be streamed.
#[derive(Debug, thiserror::Error)]
pub struct ProviderError {
    /// Whether trying again can help, and how to present it.
    pub class: RetryClass,
    /// What went wrong, for a person.
    pub message: String,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl ProviderError {
    fn new(class: RetryClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

/// The adapter for one protocol.
///
/// A protocol axum does not speak yet is an error at the point of use rather than a missing
/// catalog entry: the model exists, and saying so is more useful than pretending it does not.
fn adapter_for(api: Api) -> Option<Box<dyn Adapter + Send + Sync>> {
    match api {
        Api::AnthropicMessages => Some(Box::new(crate::api::anthropic::Anthropic)),
        _ => None,
    }
}

/// Streams turns from providers.
pub struct Client {
    http: reqwest::Client,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// A client with axum's defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                // No overall timeout: a long turn is a long turn. The connect timeout is what
                // separates "the model is thinking" from "the endpoint is not there".
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Stream one turn, handing each delta to `on_delta`.
    ///
    /// The callback rather than a returned stream: the daemon publishes each delta as it
    /// arrives and folds it into a turn, and both want to happen before the next one is read.
    pub async fn stream(
        &self,
        provider: &Provider,
        model: &Model,
        context: &Context,
        options: &Options,
        mut on_delta: impl FnMut(Delta),
    ) -> Result<(), ProviderError> {
        let Some(adapter) = adapter_for(model.api) else {
            return Err(ProviderError::new(
                RetryClass::Invalid,
                format!("axum does not speak {} yet", model.api.as_str()),
            ));
        };
        let Some(base_url) = provider.base_url.as_deref() else {
            return Err(ProviderError::new(
                RetryClass::Invalid,
                format!("{} has no endpoint configured", provider.id),
            ));
        };
        let key = provider.auth.resolve();
        if key.is_none() && !matches!(provider.auth, crate::provider::Auth::None) {
            return Err(ProviderError::new(
                RetryClass::Auth,
                format!("{}: {}", provider.id, provider.auth.requirement()),
            ));
        }

        let mut request = self.http.post(adapter.endpoint(base_url, model));
        for (name, value) in adapter.headers(key.as_deref()) {
            request = request.header(name, value);
        }
        let body = adapter.request(model, context, options);

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::new(RetryClass::Transport, e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            // Classified from the status, never from the body's prose: a provider is free to
            // reword its errors and a classifier that reads them breaks when it does.
            let class = RetryClass::of_status(status.as_u16());
            let detail = response.text().await.unwrap_or_default();
            return Err(ProviderError::new(
                class,
                format!("{} returned {status}: {}", provider.id, first_line(&detail)),
            ));
        }

        let mut parser = sse::Parser::new();
        let mut state = crate::api::StreamState::default();
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk =
                chunk.map_err(|e| ProviderError::new(RetryClass::Transport, e.to_string()))?;
            let text = String::from_utf8_lossy(&chunk);
            for event in parser.push(&text) {
                for delta in adapter.on_event(&mut state, &event) {
                    on_delta(delta);
                }
            }
        }
        if let Some(event) = parser.finish() {
            for delta in adapter.on_event(&mut state, &event) {
                on_delta(delta);
            }
        }
        Ok(())
    }
}

/// The first line of an error body, bounded.
///
/// A provider may answer with an HTML page or a stack trace; a transcript wants a sentence.
fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(300)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Modality;
    use axum_model::{Cost, Message};
    use std::collections::BTreeMap;

    fn model(api: Api) -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            provider: "p".into(),
            api,
            reasoning: false,
            input: vec![Modality::Text],
            context_window: 8192,
            max_tokens: 1024,
            cost: Cost::default(),
            thinking: BTreeMap::new(),
            compat: None,
        }
    }

    fn provider(base_url: Option<&str>, auth: crate::provider::Auth) -> Provider {
        Provider {
            id: "p".into(),
            name: "P".into(),
            base_url: base_url.map(str::to_owned),
            api: Api::AnthropicMessages,
            auth,
            compat: None,
            models: vec![model(Api::AnthropicMessages)],
        }
    }

    fn context() -> Context {
        Context {
            messages: vec![Message::user("hi")],
            ..Context::default()
        }
    }

    async fn stream_from(p: &Provider, m: &Model) -> Result<Vec<Delta>, ProviderError> {
        let mut out = Vec::new();
        Client::new()
            .stream(p, m, &context(), &Options::default(), |d| out.push(d))
            .await?;
        Ok(out)
    }

    #[tokio::test]
    async fn an_unspoken_protocol_is_refused_by_name() {
        let p = provider(Some("http://127.0.0.1:1"), crate::provider::Auth::None);
        let error = stream_from(&p, &model(Api::GoogleVertex))
            .await
            .expect_err("must fail");
        assert_eq!(error.class, RetryClass::Invalid);
        assert!(error.message.contains("google-vertex"), "{}", error.message);
    }

    #[tokio::test]
    async fn a_provider_with_no_endpoint_says_so_rather_than_dialling_nothing() {
        let p = provider(None, crate::provider::Auth::None);
        let error = stream_from(&p, &model(Api::AnthropicMessages))
            .await
            .expect_err("must fail");
        assert_eq!(error.class, RetryClass::Invalid);
        assert!(error.message.contains("no endpoint"), "{}", error.message);
    }

    #[tokio::test]
    async fn a_missing_credential_is_an_auth_error_that_names_the_variable() {
        let p = provider(
            Some("http://127.0.0.1:1"),
            crate::provider::Auth::env(&["AXUM_TEST_KEY_DEFINITELY_UNSET"]),
        );
        let error = stream_from(&p, &model(Api::AnthropicMessages))
            .await
            .expect_err("must fail");
        assert_eq!(error.class, RetryClass::Auth);
        assert!(
            error.message.contains("AXUM_TEST_KEY_DEFINITELY_UNSET"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_is_a_transport_error() {
        // Port 1 is reserved and nothing listens on it.
        let p = provider(Some("http://127.0.0.1:1"), crate::provider::Auth::None);
        let error = stream_from(&p, &model(Api::AnthropicMessages))
            .await
            .expect_err("must fail");
        assert_eq!(error.class, RetryClass::Transport);
        assert!(error.class.is_retryable());
    }

    #[test]
    fn an_error_body_is_reduced_to_a_sentence() {
        assert_eq!(first_line("\n\noverloaded\ndetail\n"), "overloaded");
        assert_eq!(first_line(&"x".repeat(1000)).len(), 300);
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn the_set_of_spoken_protocols_is_stated_rather_than_discovered() {
        // Adding an adapter should be a deliberate change to this list, not something that
        // happens quietly. The rest are refused by name at the point of use, which is a better
        // answer than a model that appears in the catalog and then does nothing.
        let spoken: Vec<&str> = Api::all()
            .into_iter()
            .filter(|api| adapter_for(*api).is_some())
            .map(Api::as_str)
            .collect();
        assert_eq!(spoken, ["anthropic-messages"]);
    }
}
