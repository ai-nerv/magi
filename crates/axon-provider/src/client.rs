//! Streaming a turn from a provider.
//!
//! The only part of this crate that does I/O. Everything it needs to know about a protocol
//! comes from an [`Adapter`]; everything it needs to know about a vendor comes from the
//! catalog. It knows neither.

use crate::api::{Adapter, Delta, Options};
use crate::model::Model;
use crate::provider::{Auth, Provider};
use crate::retry::RetryClass;

/// How many times a request is made before the failure is the answer.
///
/// Four, because the delays grow: a fifth would have the caller waiting minutes for something
/// that is plainly not coming back.
const MAX_ATTEMPTS: u32 = 4;

/// Everything one request is made of.
///
/// Gathered because the five travel together everywhere and always have: a signature that
/// lists them beside two callbacks is one nobody reads, and one where the callbacks are easy
/// to transpose.
pub struct Call<'a> {
    /// The protocol description that shapes the request and reads the stream.
    pub adapter: &'a dyn Adapter,
    /// Who is being asked.
    pub provider: &'a Provider,
    /// Which of their models.
    pub model: &'a Model,
    /// The conversation so far.
    pub context: &'a Context,
    /// What to ask for beyond it.
    pub options: &'a Options,
}

/// A failure that is being waited out rather than reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retrying {
    /// Which try just failed, counting from one.
    pub attempt: u32,
    /// How many will be made in total.
    pub max_attempts: u32,
    /// How long before the next one.
    pub delay: std::time::Duration,
    /// What went wrong, for the person watching.
    pub why: String,
}
use crate::sse;
use axon_model::Context;
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
    /// The first backoff delay; each later one grows from it.
    ///
    /// Held rather than read from a constant, because the retry policy is this client's and
    /// not the module's. A test that needs four attempts should not have to spend a minute of
    /// real time proving arithmetic that has tests of its own.
    base_delay: std::time::Duration,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// The same, waiting `base_delay` before the first retry.
    ///
    /// For tests, and for anyone who knows their endpoint recovers faster than a public one.
    #[must_use]
    pub fn with_base_delay(base_delay: std::time::Duration) -> Self {
        Self {
            base_delay,
            ..Self::new()
        }
    }

    /// A client with axon's defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                // No overall timeout: a long turn is a long turn. The connect timeout is what
                // separates "the model is thinking" from "the endpoint is not there".
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_delay: crate::retry::BASE,
        }
    }

    /// Stream one turn, handing each delta to `on_delta`.
    ///
    /// The callback rather than a returned stream: the daemon publishes each delta as it
    /// arrives and folds it into a turn, and both want to happen before the next one is read.
    pub async fn stream(
        &self,
        call: &Call<'_>,
        on_delta: impl FnMut(Delta),
    ) -> Result<(), ProviderError> {
        self.stream_reporting(call, on_delta, |_| {}).await
    }

    /// The same, saying when it is about to wait and try again.
    ///
    /// A 529 from a busy provider is routine, and so is a network blip. Both are survivable by
    /// waiting, and `retry.rs` has had the policy — classification, exponential backoff,
    /// per-request jitter, tests — since M2 with nothing calling it. A turn died on the first
    /// hiccup while the code that would have saved it sat one module away.
    ///
    /// The report is a callback rather than a return value because it happens *during*: a
    /// person watching a spinner for forty seconds needs to be told it is a wait and not a
    /// hang, and afterwards is too late to say so.
    pub async fn stream_reporting(
        &self,
        call: &Call<'_>,
        mut on_delta: impl FnMut(Delta),
        mut on_retry: impl FnMut(Retrying),
    ) -> Result<(), ProviderError> {
        let mut attempt = 1;
        loop {
            // Straight through, as they arrive. They were collected into a `Vec` and replayed
            // only on success, for a reason that was real — deltas from an attempt that then
            // failed must not be *kept*, or a turn that folded half a message and then retried
            // would show it twice. The cost was that nothing streamed at all: a three-hundred
            // word answer was fourteen seconds of spinner and then the whole text at once.
            //
            // The tension resolves the other way round. A delta reaches the caller immediately,
            // and `on_retry` is the caller's signal to **retract what it has published** before
            // the next attempt starts. Every caller that keeps deltas has to honour that; one
            // that only counts them does not care.
            let outcome = self.attempt(call, &mut on_delta).await;
            match outcome {
                Ok(()) => return Ok(()),
                Err(why) if why.class.is_retryable() && attempt < MAX_ATTEMPTS => {
                    let wait = crate::retry::backoff_from(
                        self.base_delay,
                        attempt,
                        crate::retry::seed(
                            &format!("{}/{}", call.provider.id, call.model.id),
                            attempt,
                        ),
                    );
                    on_retry(Retrying {
                        attempt,
                        max_attempts: MAX_ATTEMPTS,
                        delay: wait,
                        why: why.message.clone(),
                    });
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                Err(why) => return Err(why),
            }
        }
    }

    /// One try, with no policy about what to do if it fails.
    async fn attempt(
        &self,
        call: &Call<'_>,
        mut on_delta: impl FnMut(Delta),
    ) -> Result<(), ProviderError> {
        let Call {
            adapter,
            provider,
            model,
            context,
            options,
        } = call;
        let Some(base_url) = provider.base_url.as_deref() else {
            return Err(ProviderError::new(
                RetryClass::Invalid,
                format!("{} has no endpoint configured", provider.id),
            ));
        };
        // Resolved here rather than at catalog load, because an OAuth token has a lifetime
        // and the only moment its freshness matters is the moment it is used.
        let key = match credential(&self.http, provider).await {
            Ok(key) => key,
            Err(why) => return Err(ProviderError::new(RetryClass::Auth, why.to_string())),
        };
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

/// The credential to send, renewed if it was about to expire.
///
/// A refresh is one round trip and happens at most once an hour; a stale token is a 401 that
/// costs the turn. The exchange is not retried: if the provider will not renew, signing in
/// again is the only thing that helps, and telling the person that is more use than trying.
async fn credential(
    http: &reqwest::Client,
    provider: &Provider,
) -> Result<Option<String>, crate::oauth::Error> {
    let Auth::OAuth {
        token_url: Some(token_url),
        client_id: Some(client_id),
        ..
    } = &provider.auth
    else {
        return Ok(provider.auth.resolve());
    };

    let mut store = crate::oauth::Store::load()?;
    let tokens = store
        .get(&provider.id)
        .ok_or_else(|| crate::oauth::Error::NotSignedIn(provider.id.clone()))?;
    if !tokens.is_stale(crate::oauth::now()) {
        return Ok(Some(tokens.access.clone()));
    }
    let refresh = tokens
        .refresh
        .clone()
        .ok_or_else(|| crate::oauth::Error::Expired(provider.id.clone()))?;

    let renewed = crate::oauth::exchange(
        http,
        token_url,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", client_id),
        ],
    )
    .await?;
    let access = renewed.access.clone();
    store.put(&provider.id, renewed);
    // Best effort: a token that works but could not be written costs a refresh next time,
    // which is a great deal better than refusing to use it.
    let _ = store.save();
    Ok(Some(access))
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

impl Client {
    /// Ask for one value of a known shape, and parse it.
    ///
    /// The reusable half of structured output: everywhere axon wants a *decision* from a model
    /// rather than prose — which permissions a plan needs, how to summarise for compaction, what
    /// to title a session — is this call with a different schema.
    ///
    /// Two places the answer can arrive from, because the protocols disagree. Most put it in the
    /// response text; Anthropic has no `response_format`, so its adapter asks with a single
    /// forced tool and the value comes back as that call's arguments. Both are collected and the
    /// tool call wins, since a provider that produced one was answering the schema by
    /// construction.
    ///
    /// # Errors
    /// When the request failed, or when what came back was not the shape that was asked for —
    /// which is a fact about the model worth reporting rather than papering over.
    pub async fn value(&self, call: &Call<'_>) -> Result<serde_json::Value, ProviderError> {
        let text = std::cell::RefCell::new(String::new());
        let args = std::cell::RefCell::new(String::new());
        self.stream_reporting(
            call,
            |delta| match delta {
                Delta::Text(chunk) => text.borrow_mut().push_str(&chunk),
                Delta::ToolCallArgs(chunk) => args.borrow_mut().push_str(&chunk),
                _ => {}
            },
            // This one keeps deltas, so it retracts. Half an answer from a failed attempt
            // concatenated with a whole one from the next parses as neither.
            |_| {
                text.borrow_mut().clear();
                args.borrow_mut().clear();
            },
        )
        .await?;

        let text = text.into_inner();
        let args = args.into_inner();
        let raw = if args.trim().is_empty() { &text } else { &args };
        serde_json::from_str(raw.trim()).map_err(|why| {
            ProviderError::new(
                RetryClass::Invalid,
                format!("the answer was not the shape that was asked for: {why}"),
            )
        })
    }
}

#[cfg(test)]
mod value_tests {

    #[test]
    fn a_tool_call_answer_is_preferred_over_text() {
        // Anthropic answers a schema by calling a forced tool; anything in the text beside it is
        // commentary, and a provider that produced a call was answering by construction.
        let text = "Sure! Here is the JSON:";
        let args = r#"{"ok":true}"#;
        let raw = if args.trim().is_empty() { text } else { args };
        assert_eq!(raw, args);
    }

    #[test]
    fn text_is_used_when_there_was_no_tool_call() {
        let text = r#"{"ok":false}"#;
        let args = "";
        let raw = if args.trim().is_empty() { text } else { args };
        assert_eq!(raw, text);
    }
}
