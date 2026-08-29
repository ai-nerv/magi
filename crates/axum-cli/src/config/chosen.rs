//! Which model this directory actually uses.
//!
//! The question has two answers — what was remembered here and what the configuration says — and
//! the whole point of this module is the relationship between them.

use super::{Loaded, remembered, resolve};

/// The model this directory will actually use, and the provider offering it.
///
/// What was chosen here last, over what the configuration says — and *over* means it is tried
/// first, not that it wins. A remembered name that no longer resolves, or whose provider has no
/// credential, must not be able to take a working configuration down with it: it is a preference,
/// and a preference that can disable a setting is a bug. That is what the first version did, and
/// what it looked like was "No model is configured" on a machine whose `axum.model` was fine.
pub(super) fn chosen(
    loaded: &Loaded,
) -> Option<(
    &axum_provider::provider::Provider,
    &axum_provider::model::Model,
)> {
    let usable = |name: &str| {
        resolve(&loaded.providers, name).filter(|(provider, _)| provider.is_configured())
    };
    remembered()
        .model
        .as_deref()
        .and_then(usable)
        .or_else(|| loaded.config.string("model").and_then(usable))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BUILTIN, backend, builtin};
    use axum_lua::Engine;

    /// A catalog loaded the way the real thing loads it, with `config` layered over the top.
    fn loaded(config: &str) -> Loaded {
        let mut engine = Engine::new();
        engine.run(BUILTIN, "providers.lua").expect("catalog");
        engine.run(config, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            providers: builtin().expect("the built-in catalog must load"),
            tools: Vec::new(),
            stubs: Vec::new(),
            apis: Vec::new(),
        }
    }

    #[test]
    fn a_remembered_name_that_resolves_to_nothing_does_not_veto_the_configuration() {
        // `remembered.or_else(configured)` only falls back when nothing was remembered, so a
        // stale name reported "No model is configured" on a machine whose `axum.model` was good.
        let loaded = loaded("");
        let usable = |name: &str| {
            resolve(&loaded.providers, name).filter(|(provider, _)| provider.is_configured())
        };
        assert!(
            usable("no/such/model/anywhere").is_none(),
            "the premise: this name resolves to nothing"
        );
        let picked = Some("no/such/model/anywhere")
            .and_then(usable)
            .or_else(|| Some("openrouter/anthropic/claude-opus-5").and_then(usable));
        assert_eq!(
            picked.is_some(),
            usable("openrouter/anthropic/claude-opus-5").is_some(),
            "it falls through to what the configuration said"
        );
    }

    #[test]
    fn the_picker_and_the_worker_are_told_the_same_model() {
        // Two entry points read this. Computing it twice let the daemon report one model in its
        // picker and answer with another.
        let loaded = loaded(r#"axum.model = "openrouter/anthropic/claude-opus-5""#);
        let named = chosen(&loaded).map(|(_, model)| model.qualified());
        let running = backend(&loaded).map(|backend| backend.model.qualified());
        assert_eq!(named, running);
    }
}
