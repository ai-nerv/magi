//! Shared fixtures for the protocol tests.

use super::{LuaAdapter, engine_with_builtins};
use magi_model::{Context, Cost, Message};
use magi_provider::api::{Adapter, Delta, StreamState};
use magi_provider::model::{Api, Modality, Model};
use magi_provider::sse;
use std::collections::BTreeMap;

/// A model with nothing unusual about it.
pub fn plain_model() -> Model {
    Model {
        id: "m-1".into(),
        name: "M".into(),
        provider: "p".into(),
        api: Api::OpenAiCompletions,
        reasoning: true,
        input: vec![Modality::Text],
        context_window: 200_000,
        max_tokens: 8192,
        cost: Cost::default(),
        thinking: BTreeMap::new(),
        compat: None,
    }
}

/// A conversation with one thing in it.
pub fn plain_context() -> Context {
    Context {
        messages: vec![Message::user("hi")],
        ..Context::default()
    }
}

/// One protocol, ready to drive.
pub fn adapter(name: &str) -> LuaAdapter {
    LuaAdapter::new(engine_with_builtins().expect("builtins load"), name)
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Fold recorded events and collect what a caller would be told.
pub fn stream(adapter: &LuaAdapter, events: &[(&str, &str)]) -> Vec<Delta> {
    let mut state = StreamState::default();
    events
        .iter()
        .flat_map(|(name, data)| {
            adapter.on_event(
                &mut state,
                &sse::Event {
                    name: (*name).to_owned(),
                    data: (*data).to_owned(),
                },
            )
        })
        .collect()
}
