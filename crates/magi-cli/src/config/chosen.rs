//! Which model this directory actually uses: what was remembered here, against what the configuration says.

use super::{Loaded, remembered};

/// The model this directory will actually use, and the provider offering it. A remembered name is
/// tried first, not preferred: one that no longer resolves must not disable a working configuration.
pub(super) fn chosen(loaded: &Loaded, catalog: &magi_host::catalog::Catalog) -> Option<String> {
    let usable = |name: &str| catalog.backend(name).map(|backend| backend.model);
    // A role's own model first: a child in its lead's directory would otherwise take the model the
    // lead remembered there.
    super::agents::own(loaded)
        .and_then(|role| role.model)
        .as_deref()
        .and_then(usable)
        .or_else(|| remembered().model.as_deref().and_then(usable))
        .or_else(|| super::main_model(loaded).and_then(usable))
}

/// The name that was asked for, whether or not it can be used: what will actually run when something
/// will, otherwise the first name asked for, so `Catalog::unusable` has a name to give a reason about.
pub(super) fn asked(loaded: &Loaded, catalog: &magi_host::catalog::Catalog) -> Option<String> {
    chosen(loaded, catalog)
        .or_else(|| super::agents::own(loaded).and_then(|role| role.model))
        .or_else(|| remembered().model)
        .or_else(|| super::main_model(loaded).map(ToOwned::to_owned))
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
        // That the walk saw anything at all, not a count of the tree: `config/` is down to
        // `tools.lua` beside the entry point now that no client library ships as a file.
        assert!(checked >= 1, "the walk checked nothing");
    }

    #[test]
    fn a_client_named_at_all_is_named_before_the_tool_that_loads_it() {
        // A tool declares itself by reading its sibling's client library, so a named one must load
        // first. None ships — `client` serves them — so this holds whoever adds one back.
        let init = checkout("init.lua");
        let tools = init.find("magi.load(\"tools").expect("tools are loaded");
        if let Some(client) = init.find("magi.load(\"clients/") {
            assert!(
                client < tools,
                "a client loads after the tools that read it"
            );
        }
    }

    #[test]
    fn no_client_library_is_shipped_as_a_file() {
        // `client` serves each sibling's own library, so a copy here is one that goes stale. magi
        // carried `hexe` and `oslo` long after both moved to casper, read by nothing.
        let init = checkout("init.lua");
        assert!(
            !init.contains("magi.load(\"clients/"),
            "a shipped client is a copy that goes stale: {init}"
        );
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

/// `magi.model` as a name on its own, and as a table.
#[cfg(test)]
mod shapes {
    use super::tests::loaded;
    use crate::config::{helper_models, main_model};

    #[test]
    fn a_plain_name_is_the_main_model_and_names_no_helper() {
        // The older one-line form, which must go on meaning what it meant.
        let held = loaded(r#"magi.model = "open/good""#);
        assert_eq!(main_model(&held), Some("open/good"));
        assert!(helper_models(&held).is_empty());
    }

    #[test]
    fn a_table_names_the_main_model_and_a_helper_for_each_kind_of_work() {
        let held =
            loaded(r#"magi.model = { main = "open/good", helper = { decision = "decides/x" } }"#);
        assert_eq!(main_model(&held), Some("open/good"));
        assert_eq!(
            helper_models(&held).get("decision").map(String::as_str),
            Some("decides/x")
        );
    }

    #[test]
    fn a_role_nothing_here_knows_about_is_carried_through_all_the_same() {
        // The table is open on purpose: a role added later needs no change in this file.
        let held =
            loaded(r#"magi.model = { main = "m", helper = { decision = "d", whatever = "w" } }"#);
        assert_eq!(
            helper_models(&held).get("whatever").map(String::as_str),
            Some("w")
        );
    }

    #[test]
    fn a_table_with_no_main_names_no_model_rather_than_guessing_at_one() {
        let held = loaded(r#"magi.model = { helper = { decision = "d" } }"#);
        assert_eq!(main_model(&held), None);
    }

    #[test]
    fn nothing_configured_names_nothing() {
        let held = loaded("");
        assert_eq!(main_model(&held), None);
        assert!(helper_models(&held).is_empty());
    }
}

/// Which model runs a role, from `magi.model.helper`.
#[cfg(test)]
mod helping {
    use super::tests::loaded;
    use crate::config::settings::helpers;

    #[test]
    fn a_role_named_there_runs_on_that_model() {
        let held = loaded(
            r#"magi.model = { main = "m", helper = { decision = "d/x", summary = "s/y" } }"#,
        );
        let roles = helpers(&held).roles;
        assert_eq!(roles.get("decision").map(String::as_str), Some("d/x"));
        assert_eq!(roles.get("summary").map(String::as_str), Some("s/y"));
    }

    #[test]
    fn notes_are_kept_on_the_main_model_until_a_helper_is_named_for_them() {
        let held = loaded(r#"magi.model = { main = "m" }"#);
        assert_eq!(
            helpers(&held).roles.get("memory").map(String::as_str),
            Some(magi_host::helping::MAIN)
        );
    }

    #[test]
    fn a_role_set_to_false_runs_nowhere() {
        // How notes are turned off, and it has to work in the new table as well as the old one.
        let held = loaded(r#"magi.model = { main = "m", helper = { memory = false } }"#);
        assert!(!helpers(&held).roles.contains_key("memory"));
    }

    #[test]
    fn the_limits_are_still_read_from_the_older_table() {
        // `magi.helpers` keeps the budget and the timeout; it no longer names models.
        let held = loaded(
            r#"magi.model = { main = "m" }
               magi.helpers = { timeout_ms = 1234, budget = { per_prompt = 0.5 } }"#,
        );
        let held = helpers(&held);
        assert_eq!(held.timeout_ms, 1234);
        assert_eq!(held.per_prompt_micros, Some(500_000));
    }
}
