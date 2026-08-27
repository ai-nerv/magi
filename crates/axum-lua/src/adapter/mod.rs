//! A wire protocol, described in Lua.
//!
//! The only implementation of [`Adapter`] there is. `axum-provider` owns the contract and moves
//! the bytes; every protocol — Anthropic's Messages, OpenAI's Completions, the eight others —
//! is a table of functions in `lua/apis/*.lua`, registered like anything else.
//!
//! A protocol is a description. Describing one should not need a rebuild, and a person with a
//! private endpoint that speaks something slightly different should be able to say so.

use crate::Engine;
use axum_model::Context;
use axum_provider::api::{Adapter, Delta, Options, StreamState};
use axum_provider::model::Model;
use axum_provider::sse;
use std::cell::RefCell;

/// One registered protocol, driven through the VM.
pub struct LuaAdapter {
    /// The VM holding the description. Borrowed mutably per call, so it is not shared.
    engine: RefCell<Engine>,
    /// Which protocol, by the name it registered under.
    name: String,
}

impl LuaAdapter {
    /// Take ownership of an engine and speak `name` through it.
    ///
    /// # Errors
    /// When nothing registered under that name.
    pub fn new(mut engine: Engine, name: &str) -> Result<Self, String> {
        let known = engine.apis();
        if !known.iter().any(|a| a == name) {
            return Err(format!(
                "no protocol named {name:?} is registered; axum knows {}",
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            ));
        }
        Ok(Self {
            engine: RefCell::new(engine),
            name: name.to_owned(),
        })
    }

    fn call(&self, method: &str, args: &[serde_json::Value]) -> Option<serde_json::Value> {
        self.engine.borrow_mut().call_api(&self.name, method, args)
    }
}

impl Adapter for LuaAdapter {
    fn endpoint(&self, base_url: &str, model: &Model) -> String {
        self.call(
            "endpoint",
            &[
                serde_json::json!(base_url.trim_end_matches('/')),
                serde_json::to_value(model).unwrap_or_default(),
            ],
        )
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| base_url.to_owned())
    }

    fn headers(&self, key: Option<&str>) -> Vec<(String, String)> {
        // Content type is set here rather than in every protocol: all ten post JSON, and a
        // description that had to say so would be repeating the transport's own decision.
        let mut out = vec![("content-type".to_owned(), "application/json".to_owned())];
        let Some(fields) = self
            .call("headers", &[serde_json::json!(key)])
            .and_then(|v| v.as_object().cloned())
        else {
            return out;
        };
        for (name, value) in fields {
            if let Some(value) = value.as_str() {
                out.push((name, value.to_owned()));
            }
        }
        out
    }

    fn request(&self, model: &Model, context: &Context, options: &Options) -> serde_json::Value {
        self.call(
            "request",
            &[
                serde_json::to_value(model).unwrap_or_default(),
                serde_json::to_value(context).unwrap_or_default(),
                serde_json::to_value(options).unwrap_or_default(),
            ],
        )
        .unwrap_or(serde_json::Value::Null)
    }

    fn on_event(&self, state: &mut StreamState, event: &sse::Event) -> Vec<Delta> {
        let answer = self.call(
            "on_event",
            &[
                serde_json::json!({ "scratch": state.scratch, "usage": state.usage }),
                serde_json::json!({ "name": event.name, "data": event.data }),
            ],
        );
        let Some(answer) = answer else {
            return Vec::new();
        };

        // The protocol hands back what it wants remembered and what the caller should be told.
        // Remembering is the adapter's business; this only carries it between events.
        if let Some(scratch) = answer.get("scratch") {
            state.scratch = scratch.clone();
        }
        if let Some(usage) = answer
            .get("usage")
            .and_then(|u| serde_json::from_value(u.clone()).ok())
        {
            state.usage = usage;
        }
        answer
            .get("deltas")
            .and_then(|d| d.as_array())
            .map(|deltas| deltas.iter().filter_map(delta_from_json).collect())
            .unwrap_or_default()
    }
}

/// One delta, as a protocol described it.
///
/// An unrecognised kind is dropped rather than fatal: a protocol may learn to report something
/// this build has no vocabulary for, and losing that is better than losing the turn.
fn delta_from_json(value: &serde_json::Value) -> Option<Delta> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Some(match value.get("kind").and_then(|k| k.as_str())? {
        "text" => Delta::Text(text("text")),
        "thinking" => Delta::Thinking(text("thinking")),
        "signature" => Delta::Signature(text("signature")),
        "tool_call_start" => Delta::ToolCallStart {
            id: text("id"),
            name: text("name"),
        },
        "tool_call_args" => Delta::ToolCallArgs(text("arguments")),
        "usage" => Delta::Usage(serde_json::from_value(value.get("usage")?.clone()).ok()?),
        "stop" => Delta::Stop(serde_json::from_value(value.get("reason")?.clone()).ok()?),
        _ => return None,
    })
}

/// The protocol descriptions axum ships.
///
/// Compiled in so a fresh install speaks something, and registered through the same registrar a
/// user's own file would use — a private protocol is an extra file, not a fork.
pub const BUILTIN: &[(&str, &str)] = &[
    (
        "anthropic-messages",
        include_str!("../../../../config/apis/anthropic-messages.lua"),
    ),
    (
        "openai-completions",
        include_str!("../../../../config/apis/openai-completions.lua"),
    ),
    (
        "openai-responses",
        include_str!("../../../../config/apis/openai-responses.lua"),
    ),
    ("google", include_str!("../../../../config/apis/google.lua")),
    (
        "pi-messages",
        include_str!("../../../../config/apis/pi-messages.lua"),
    ),
];

/// An engine with every built-in protocol registered.
pub fn engine_with_builtins() -> Result<Engine, crate::LuaError> {
    let mut engine = Engine::new();
    for (name, source) in BUILTIN {
        engine.run(source, name)?;
    }
    Ok(engine)
}

/// Protocols axum knows of but does not speak, and why.
///
/// Named rather than simply absent. A model that appears in the catalog and then does nothing
/// is worse than one that says what is missing — and a gap with a stated reason is a task,
/// while a gap without one is a mystery.
pub const UNSPOKEN: &[(&str, &str)] = &[(
    "bedrock-converse-stream",
    "Bedrock frames its stream as binary AWS eventstream records rather than server-sent \
     events, so it needs a decoder in the transport before a description can read it. That \
     is Rust work, not a protocol file.",
)];

/// Why a protocol is not spoken, if it is a known gap.
#[must_use]
pub fn why_unspoken(api: &str) -> Option<&'static str> {
    UNSPOKEN
        .iter()
        .find(|(name, _)| *name == api)
        .map(|(_, why)| *why)
}

#[cfg(test)]
mod protocols;
#[cfg(test)]
mod support;
#[cfg(test)]
mod tests;
