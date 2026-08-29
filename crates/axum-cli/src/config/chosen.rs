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

/// The name that was asked for, whether or not it can be used.
///
/// Not the same question as [`chosen`], and the difference is the whole of a bug. `chosen` answers
/// "what will run", so it is `None` when the provider has no credential — and that is exactly the
/// case the daemon's refusal exists to explain. Fed a `None`, it fell back to "No model is
/// configured", which is false: one *is* configured, its key is not set, and saying so is the
/// difference between a person setting a variable and a person believing the thing is broken.
///
/// So: what will actually run when something will, and otherwise the first name that was asked
/// for, so `Catalog::unusable` has a name to give a reason about.
pub(super) fn asked(loaded: &Loaded) -> Option<String> {
    chosen(loaded)
        .map(|(_, model)| model.qualified())
        .or_else(|| remembered().model)
        .or_else(|| loaded.config.string("model").map(ToOwned::to_owned))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::config::{BUILTIN, backend, builtin};
    use axum_lua::Engine;

    /// A catalog loaded the way the real thing loads it, with `config` layered over the top.
    pub(super) fn loaded(config: &str) -> Loaded {
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

#[cfg(test)]
mod asked_tests {
    use super::tests::loaded;
    use super::*;

    #[test]
    fn a_configured_model_is_named_even_when_its_provider_has_no_key() {
        // The regression: pointing `Catalog::chosen` at what *runs* made it `None` in exactly
        // the case the daemon's refusal exists to explain, and "No model is configured" came
        // back on a machine whose `axum.model` was set and whose key merely was not.
        let loaded = loaded(r#"axum.model = "anthropic/claude-opus-5""#);
        let name = "anthropic/claude-opus-5";
        let keyless = resolve(&loaded.providers, name).is_some_and(|(p, _)| !p.is_configured());
        if !keyless {
            return; // A machine with the key set has nothing to say here.
        }
        assert!(chosen(&loaded).is_none(), "the premise: it cannot be used");
        assert_eq!(
            asked(&loaded).as_deref(),
            Some(name),
            "but the refusal still has a name to give a reason about"
        );
    }

    #[test]
    fn what_runs_is_what_is_named_when_something_runs() {
        let loaded = loaded(r#"axum.model = "openrouter/anthropic/claude-opus-5""#);
        if let Some((_, model)) = chosen(&loaded) {
            assert_eq!(asked(&loaded), Some(model.qualified()));
        }
    }
}
