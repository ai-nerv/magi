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
    /// The VM holding the description.
    ///
    /// Shared with the tools that run in it: one thread, one VM, and a protocol and a tool
    /// that disagreed about which one they were in would be a bug nobody could see.
    engine: std::rc::Rc<RefCell<Engine>>,
    /// Which protocol, by the name it registered under.
    name: String,
}

impl LuaAdapter {
    /// Take ownership of an engine and speak `name` through it.
    ///
    /// # Errors
    /// When nothing registered under that name.
    pub fn from_shared(
        engine: std::rc::Rc<std::cell::RefCell<Engine>>,
        name: &str,
    ) -> Result<Self, String> {
        let known = engine.borrow_mut().apis();
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
            engine,
            name: name.to_owned(),
        })
    }

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
            engine: std::rc::Rc::new(RefCell::new(engine)),
            name: name.to_owned(),
        })
    }

    fn call(&self, method: &str, args: &[serde_json::Value]) -> Option<serde_json::Value> {
        self.engine.borrow_mut().call_api(&self.name, method, args)
    }
}

/// The model as a protocol description should see it.
///
/// Its `compat` is replaced by the fully resolved dialect, so a description reads
/// `compat.thinking_format` and never `compat.thinking_format or "openai"`. Every default
/// lives in `axum_provider::compat::Resolved` and nowhere else — ten descriptions each
/// carrying their own copy is ten places for one of them to fall behind.
fn described(model: &Model) -> serde_json::Value {
    let mut value = serde_json::to_value(model).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        let resolved = axum_provider::compat::resolve(model.compat);
        object.insert(
            "compat".to_owned(),
            serde_json::to_value(resolved).unwrap_or_default(),
        );
    }
    value
}

/// The options as this model can actually take them.
///
/// `thinking` is settled here rather than in a description: the catalog can say a model cannot
/// do a level at all, which is not the same as having no opinion about it — and in Lua a key
/// with no value and no key at all are indistinguishable, so the difference cannot survive the
/// crossing. Ten descriptions each re-deriving it would be ten chances to lose it.
fn asked(model: &Model, options: &Options) -> serde_json::Value {
    let mut value = serde_json::to_value(options).unwrap_or_default();
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    match effective_thinking(model, options) {
        Some(level) => {
            object.insert("thinking".to_owned(), serde_json::Value::String(level));
        }
        None => {
            object.remove("thinking");
        }
    }
    value
}

/// What to ask this model for, or nothing.
///
/// Nothing when it does not reason — a model sent `reasoning_effort` that has none answers 400,
/// and the request that fails is the one somebody just typed.
fn effective_thinking(model: &Model, options: &Options) -> Option<String> {
    if !model.reasoning {
        return None;
    }
    let level = options.thinking?;
    if level == axum_model::ThinkingLevel::Off {
        return None;
    }
    match model.thinking.get(&level) {
        // Named: this model calls that level something else.
        Some(Some(name)) => Some(name.clone()),
        // Present and empty: it cannot do this level. Asking anyway is a refusal.
        Some(None) => None,
        // Unmapped, which is the ordinary case: the provider's own word for it.
        None => serde_json::to_value(level)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned)),
    }
}

impl Adapter for LuaAdapter {
    fn endpoint(&self, base_url: &str, model: &Model) -> String {
        self.call(
            "endpoint",
            &[
                serde_json::json!(base_url.trim_end_matches('/')),
                described(model),
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
                described(model),
                serde_json::to_value(context).unwrap_or_default(),
                asked(model, options),
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

/// An engine with the protocols in `path` registered.
///
/// A path, not a compiled-in copy. A protocol description is configuration: it changes without
/// the binary changing, and a binary that carries one is a binary you have to rebuild to fix a
/// wire format.
pub fn engine_with(sources: &[(String, String)]) -> Result<Engine, crate::LuaError> {
    let mut engine = Engine::new();
    for (name, source) in sources {
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

/// The protocol descriptions in the checkout, read at run time. **For tests.**
///
/// The product reads its configuration from the config directory and carries no copy. A test
/// still needs a real protocol to drive, so this is the one place that knows where the tree is —
/// and it is a helper, not a path anything shipped depends on.
///
/// # Errors
/// When the checkout's `config/apis.lua` cannot be read.
pub fn shipped_apis() -> Result<Vec<(String, String)>, crate::LuaError> {
    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/apis.lua");
    let source = std::fs::read_to_string(PATH).map_err(|source| crate::LuaError::Io {
        file: PATH.to_owned(),
        source,
    })?;
    Ok(vec![("apis".to_owned(), source)])
}

/// An engine with those protocols registered. **For tests.**
///
/// # Errors
/// When the descriptions cannot be read or do not load.
pub fn engine_with_builtins() -> Result<Engine, crate::LuaError> {
    engine_with(&shipped_apis()?)
}

#[cfg(test)]
mod protocols;
#[cfg(test)]
mod support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod thinking_tests {
    use super::*;
    use axum_model::ThinkingLevel;
    use std::collections::BTreeMap;

    fn model(reasoning: bool, thinking: BTreeMap<ThinkingLevel, Option<String>>) -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            provider: "p".into(),
            api: axum_provider::model::Api::OpenAiCompletions,
            reasoning,
            input: Vec::new(),
            context_window: 1000,
            max_tokens: 100,
            cost: axum_model::Cost::default(),
            thinking,
            compat: None,
        }
    }

    fn wanting(level: ThinkingLevel) -> Options {
        Options {
            schema: None,
            thinking: Some(level),
            max_tokens: None,
        }
    }

    #[test]
    fn a_model_that_does_not_reason_is_never_asked_to() {
        // Sent `reasoning_effort` it answers 400, and the request that fails is the one
        // somebody just typed.
        let asked = asked(
            &model(false, BTreeMap::new()),
            &wanting(ThinkingLevel::High),
        );
        assert!(asked.get("thinking").is_none(), "{asked}");
    }

    #[test]
    fn an_unmapped_level_goes_through_as_the_provider_names_it() {
        let asked = asked(&model(true, BTreeMap::new()), &wanting(ThinkingLevel::High));
        assert_eq!(asked["thinking"], "high");
    }

    #[test]
    fn a_model_may_call_a_level_something_else() {
        let mut map = BTreeMap::new();
        map.insert(ThinkingLevel::High, Some("deep".to_owned()));
        let asked = asked(&model(true, map), &wanting(ThinkingLevel::High));
        assert_eq!(asked["thinking"], "deep");
    }

    #[test]
    fn a_level_the_model_refuses_is_not_asked_for() {
        // Present with no value means "cannot do this one", which has to stay tellable apart
        // from "no opinion" — and in Lua it cannot, which is why this is settled here.
        let mut map = BTreeMap::new();
        map.insert(ThinkingLevel::Max, None);
        let asked = asked(&model(true, map), &wanting(ThinkingLevel::Max));
        assert!(asked.get("thinking").is_none(), "{asked}");
    }

    #[test]
    fn off_is_not_a_level_to_ask_for() {
        let asked = asked(&model(true, BTreeMap::new()), &wanting(ThinkingLevel::Off));
        assert!(asked.get("thinking").is_none(), "{asked}");
    }

    #[test]
    fn asking_for_nothing_asks_for_nothing() {
        let asked = asked(&model(true, BTreeMap::new()), &Options::default());
        assert!(asked.get("thinking").is_none(), "{asked}");
    }
}
