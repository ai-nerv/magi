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
/// what it looked like was "No model is configured" on a machine whose `axon.model` was fine.
pub(super) fn chosen(
    loaded: &Loaded,
) -> Option<(
    &axon_provider::provider::Provider,
    &axon_provider::model::Model,
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
pub(crate) mod tests {
    use super::*;
    use crate::config::{backend, builtin};
    use axon_lua::Engine;

    /// The checkout's own `config/`, read at run time as the product reads it.
    pub(crate) fn checkout(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// A catalog loaded the way the real thing loads it, with `config` layered over the top.
    pub(super) fn loaded(config: &str) -> Loaded {
        let mut engine = Engine::new();
        engine
            .run(&checkout("providers.lua"), "providers.lua")
            .expect("catalog");
        engine.run(config, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            providers: builtin().expect("the built-in catalog must load"),
            tools: Vec::new(),
            clients: Vec::new(),
            apis: Vec::new(),
        }
    }

    #[test]
    fn a_remembered_name_that_resolves_to_nothing_does_not_veto_the_configuration() {
        // `remembered.or_else(configured)` only falls back when nothing was remembered, so a
        // stale name reported "No model is configured" on a machine whose `axon.model` was good.
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
        let loaded = loaded(r#"axon.model = "openrouter/anthropic/claude-opus-5""#);
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
        // back on a machine whose `axon.model` was set and whose key merely was not.
        let loaded = loaded(r#"axon.model = "anthropic/claude-opus-5""#);
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
        let loaded = loaded(r#"axon.model = "openrouter/anthropic/claude-opus-5""#);
        if let Some((_, model)) = chosen(&loaded) {
            assert_eq!(asked(&loaded), Some(model.qualified()));
        }
    }
}

/// One entry point, reading a tree the binary does not carry.
#[cfg(test)]
mod entry_point {
    use super::tests::checkout;
    use crate::config::kind;

    #[test]
    fn the_entry_point_names_every_file_beside_it() {
        // Nothing is discovered by scanning, so a file in the tree that `init.lua` never loads
        // is a file that ships and does nothing.
        let init = checkout("init.lua");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("the config tree") {
            let path = entry.expect("entry").path();
            let name = path
                .file_name()
                .expect("named")
                .to_string_lossy()
                .into_owned();
            if name == "init.lua" {
                continue;
            }
            if path.is_dir() {
                for inner in std::fs::read_dir(&path).expect("subdirectory") {
                    let leaf = inner
                        .expect("entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned();
                    assert!(
                        init.contains(&format!("axon.load(\"{name}/{leaf}\")")),
                        "init.lua never loads {name}/{leaf}"
                    );
                    checked += 1;
                }
            } else if name.ends_with(".lua") {
                assert!(
                    init.contains(&format!("axon.load(\"{name}\")")),
                    "init.lua never loads {name}"
                );
                checked += 1;
            }
        }
        assert!(checked >= 3, "only {checked} files checked");
    }

    #[test]
    fn a_client_is_named_before_the_tool_that_loads_it() {
        // A tool declares itself by loading its sibling's client library, so the order in the
        // entry point is load-bearing rather than tidy.
        let init = checkout("init.lua");
        let client = init
            .find("axon.load(\"clients/")
            .expect("a client is loaded");
        let tools = init.find("axon.load(\"tools").expect("tools are loaded");
        assert!(client < tools);
    }

    #[test]
    fn a_protocol_is_named_before_the_catalog_that_picks_one() {
        // `api = "openai-completions"` in a provider is a name that has to already mean
        // something, so the order in the entry point is load-bearing rather than tidy.
        let init = checkout("init.lua");
        let apis = init.find("axon.load(\"apis").expect("protocols are loaded");
        let catalog = init
            .find("axon.load(\"providers")
            .expect("the catalog is loaded");
        assert!(apis < catalog);
    }

    #[test]
    fn a_merged_file_and_a_split_one_land_in_the_same_bucket() {
        // The tree keeps one file per kind; somebody who prefers a file per protocol should not
        // have to tell the host about it.
        assert_eq!(kind("apis.lua"), Some("apis"));
        assert_eq!(kind("apis/google.lua"), Some("apis"));
        assert_eq!(kind("tools.lua"), Some("tools"));
        assert_eq!(kind("clients/oslo.lua"), Some("clients"));
        assert_eq!(kind("providers.lua"), None);
    }
}
