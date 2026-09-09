//! Which model this directory actually uses: what was remembered here, against what the configuration says.

use super::{Loaded, remembered};

/// The model this directory will actually use, and the provider offering it. A remembered name is
/// tried first, not preferred: one that no longer resolves must not disable a working configuration.
pub(super) fn chosen(loaded: &Loaded, catalog: &magi_host::catalog::Catalog) -> Option<String> {
    let usable = |name: &str| catalog.backend(name).map(|backend| backend.model);
    remembered()
        .model
        .as_deref()
        .and_then(usable)
        .or_else(|| loaded.config.string("model").and_then(usable))
}

/// The name that was asked for, whether or not it can be used: what will actually run when something
/// will, otherwise the first name asked for, so `Catalog::unusable` has a name to give a reason about.
pub(super) fn asked(loaded: &Loaded, catalog: &magi_host::catalog::Catalog) -> Option<String> {
    chosen(loaded, catalog)
        .or_else(|| remembered().model)
        .or_else(|| loaded.config.string("model").map(ToOwned::to_owned))
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::backend;
    use magi_lua::Engine;
    use magi_proto::ask::Card;

    /// The checkout's own `config/`, read at run time as the product reads it.
    pub(crate) fn checkout(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// A config, loaded the way the real thing loads one.
    pub(super) fn loaded(config: &str) -> Loaded {
        let mut engine = Engine::new();
        engine.run(config, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
        }
    }

    /// Two cards: one that can be used and one whose key is not set.
    pub(super) fn catalog() -> magi_host::catalog::Catalog {
        let card = |id: &str, ready: bool| Card {
            id: id.to_owned(),
            provider: id.split('/').next().unwrap_or_default().to_owned(),
            name: id.split_once('/').map_or(id, |(_, rest)| rest).to_owned(),
            api: "openai-completions".to_owned(),
            context_window: Some(1000),
            max_output: None,
            reasons: false,
            ready,
            needs: (!ready).then(|| "MAGI_TEST_NOT_SET".to_owned()),
        };
        magi_host::catalog::Catalog {
            cards: vec![card("open/good", true), card("paid/keyless", false)],
            ..magi_host::catalog::Catalog::empty()
        }
    }

    #[test]
    fn a_remembered_name_that_resolves_to_nothing_does_not_veto_the_configuration() {
        // A stale remembered name must not mask a `magi.model` that is good.
        let held = catalog();
        let usable = |name: &str| held.backend(name).map(|b| b.model);
        assert!(
            usable("no/such/model/anywhere").is_none(),
            "the premise: this name resolves to nothing"
        );
        let picked = Some("no/such/model/anywhere")
            .and_then(usable)
            .or_else(|| Some("open/good").and_then(usable));
        assert_eq!(picked.as_deref(), Some("open/good"));
    }

    #[test]
    fn the_picker_and_the_worker_are_told_the_same_model() {
        // Two entry points read this; computing it twice let the picker and the answer disagree.
        let held = catalog();
        let loaded = loaded(r#"magi.model = "open/good""#);
        let named = chosen(&loaded, &held);
        let running = backend(&held).map(|backend| backend.model);
        assert_eq!(named, running.or(Some("open/good".to_owned())));
    }
}

#[cfg(test)]
mod asked_tests {
    use super::tests::{catalog, loaded};
    use super::*;

    #[test]
    fn a_configured_model_is_named_even_when_its_provider_has_no_key() {
        // A configured model whose key is unset must still be named, not reported as absent.
        let held = catalog();
        let loaded = loaded(r#"magi.model = "paid/keyless""#);
        assert!(
            chosen(&loaded, &held).is_none(),
            "the premise: it cannot be used"
        );
        assert_eq!(
            asked(&loaded, &held).as_deref(),
            Some("paid/keyless"),
            "but the refusal still has a name to give a reason about"
        );
    }

    #[test]
    fn what_runs_is_what_is_named_when_something_runs() {
        let held = catalog();
        let loaded = loaded(r#"magi.model = "open/good""#);
        assert_eq!(chosen(&loaded, &held), asked(&loaded, &held));
    }
}

/// One entry point, reading a tree the binary does not carry.
#[cfg(test)]
mod entry_point {
    use super::tests::checkout;
    use crate::config::kind;

    #[test]
    fn the_entry_point_names_every_file_beside_it() {
        // The shipped tree is not a plugin directory: `config/` is read through `magi.load` alone,
        // and what is discovered lives under `plugin/` and `pack/`.
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
                        init.contains(&format!("magi.load(\"{name}/{leaf}\")")),
                        "init.lua never loads {name}/{leaf}"
                    );
                    checked += 1;
                }
            } else if name.ends_with(".lua") {
                assert!(
                    init.contains(&format!("magi.load(\"{name}\")")),
                    "init.lua never loads {name}"
                );
                checked += 1;
            }
        }
        assert!(checked >= 3, "only {checked} files checked");
    }

    #[test]
    fn a_client_is_named_before_the_tool_that_loads_it() {
        // A tool declares itself by loading its sibling's client library; entry-point order matters.
        let init = checkout("init.lua");
        let client = init
            .find("magi.load(\"clients/")
            .expect("a client is loaded");
        let tools = init.find("magi.load(\"tools").expect("tools are loaded");
        assert!(client < tools);
    }

    #[test]
    fn the_entry_point_names_no_model_of_its_own() {
        // Protocols and providers moved to melchior; shipping them here would be a second catalog.
        let init = checkout("init.lua");
        assert!(!init.contains("magi.load(\"apis"), "{init}");
        assert!(!init.contains("magi.load(\"providers"), "{init}");
    }

    #[test]
    fn a_merged_file_and_a_split_one_land_in_the_same_bucket() {
        // The tree keeps one file per kind; a file per protocol needs no host change.
        assert_eq!(kind("apis.lua"), Some("apis"));
        assert_eq!(kind("apis/google.lua"), Some("apis"));
        assert_eq!(kind("tools.lua"), Some("tools"));
        assert_eq!(kind("clients/oslo.lua"), Some("clients"));
        assert_eq!(kind("providers.lua"), None);
    }
}
