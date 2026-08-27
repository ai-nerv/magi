//! Streaming a turn from a provider.
//!
//! The only part of this crate that does I/O. Everything it needs to know about a protocol
//! comes from an [`Adapter`]; everything it needs to know about a vendor comes from the
//! catalog. It knows neither.

use crate::api::{Adapter, Delta, Options};
use crate::model::Model;
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
        adapter: &dyn Adapter,
        provider: &Provider,
        model: &Model,
        context: &Context,
        options: &Options,
        mut on_delta: impl FnMut(Delta),
    ) -> Result<(), ProviderError> {
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
            // Classified from the status and, for one case, the body. A provider is free to
            // reword its errors, so reading prose is a last resort — but a context-window
            // overflow arrives as an ordinary 400 and nothing else distinguishes it from a
            // malformed request. One is worth compacting for; the other never will be.
            let detail = response.text().await.unwrap_or_default();
            let class = RetryClass::of(status.as_u16(), &detail);
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

    #[test]
    fn an_error_body_is_reduced_to_a_sentence() {
        assert_eq!(first_line("\n\noverloaded\ndetail\n"), "overloaded");
        assert_eq!(first_line(&"x".repeat(1000)).len(), 300);
        assert_eq!(first_line(""), "");
    }
}
