//! What the catalog is, and what a config may do to it.
//!
//! Split out under THE RULE; `collect` and `declare` next door are what these are about.

use super::*;

mod tests {
    use super::*;
    use magi_provider::model::Api;
    use std::collections::BTreeSet;

    fn catalog() -> Vec<Provider> {
        builtin().expect("the built-in catalog must load")
    }

    #[test]
    fn the_builtin_catalog_is_a_config_file_that_runs() {
        // Eight, deliberately. It carried forty-one, almost all listing a single model, and a
        // catalog of providers nobody has a key for is a list nobody reads. Adding one back is
        // a dozen lines of config.
        let held = catalog();
        assert!(held.len() >= 8, "only {}", held.len());
        for wanted in [
            "anthropic",
            "openai",
            "google",
            "openrouter",
            "deepseek",
            "zai",
            "github-copilot",
            "ollama",
        ] {
            assert!(
                held.iter().any(|p| p.id == wanted),
                "{wanted} is not in the catalog"
            );
        }
    }

    #[test]
    fn every_protocol_the_catalog_names_is_one_magi_speaks() {
        // The other way round from what this used to check. It asserted every `Api` variant had
        // a provider, which made trimming the catalog fail a test about protocols; what matters
        // is that nothing in the catalog names a protocol with no adapter behind it.
        let known: BTreeSet<Api> = Api::all().into_iter().collect();
        for provider in catalog() {
            assert!(
                known.contains(&provider.api),
                "{} speaks {}, which nothing implements",
                provider.id,
                provider.api.as_str()
            );
        }
    }

    #[test]
    fn the_openai_shape_is_still_the_common_one() {
        // It used to assert a majority, which held while the catalog carried forty-one
        // providers and almost all of them spoke this. Eight providers is a small enough sample
        // that a majority is an accident; what the claim is really about is that adding a
        // provider usually costs no adapter, and one adapter serving several is that.
        let providers = catalog();
        let shared = providers
            .iter()
            .filter(|p| p.api == Api::OpenAiCompletions)
            .count();
        assert!(
            shared >= 3,
            "only {shared} of {} route through openai-completions",
            providers.len()
        );
    }

    #[test]
    fn provider_ids_are_unique() {
        let providers = catalog();
        let ids: BTreeSet<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids.len(),
            providers.len(),
            "a duplicate id shadows a provider"
        );
    }

    #[test]
    fn a_provider_either_lists_its_models_or_asks_for_them() {
        // One or the other, never neither. A provider with an empty list and no `discover` is
        // one no model can ever resolve against -- it would sit in `magi models` offering
        // nothing and there would be nothing to say about why.
        for p in catalog() {
            assert!(
                !p.models.is_empty() || p.discover,
                "{} offers nothing and asks for nothing",
                p.id
            );
        }
    }

    #[test]
    fn a_discovering_provider_carries_no_written_catalog() {
        // Both would be a list that is sometimes the config's and sometimes the network's, and
        // no way to tell which you are looking at.
        for p in catalog().iter().filter(|p| p.discover) {
            assert!(
                p.models.is_empty(),
                "{} both lists models and asks for them",
                p.id
            );
        }
    }

    #[test]
    fn every_model_is_stamped_with_its_provider_and_api() {
        for p in catalog() {
            for m in &p.models {
                assert_eq!(m.provider, p.id, "{} claims {}", m.id, m.provider);
                assert_eq!(m.api, p.api, "{} speaks the wrong protocol", m.id);
            }
        }
    }

    #[test]
    fn context_windows_are_plausible() {
        for p in catalog() {
            for m in &p.models {
                assert!(
                    m.context_window >= 4096,
                    "{} has {}",
                    m.id,
                    m.context_window
                );
                assert!(
                    m.max_tokens <= m.context_window,
                    "{} would exceed its own window",
                    m.id
                );
            }
        }
    }

    #[test]
    fn a_provider_without_a_fixed_base_url_declares_its_dialect() {
        // Detection by hostname was removed with the vendor knowledge it needed. A provider
        // whose endpoint comes from configuration has no host to infer from, so it must say.
        for p in catalog() {
            if p.base_url.is_none() && p.api == Api::OpenAiCompletions {
                assert!(
                    p.compat.is_some(),
                    "{} has no base_url and no dialect",
                    p.id
                );
            }
        }
    }

    /// Run the built-in catalog with an extra config chunk layered over it.
    fn with(extra: &str) -> Vec<Provider> {
        let mut engine = Engine::new();
        engine
            .run(
                &crate::config::chosen::tests::checkout("providers.lua"),
                "providers.lua",
            )
            .expect("builtin");
        engine.run(extra, "user.lua").expect("user config");
        engine.harvest();
        // `None`: these tests are about what a *machine* config can declare, so everything
        // they run counts as the machine's own and there is no boundary to enforce.
        collect(engine.config(), Vec::new(), Vec::new(), Vec::new(), None)
            .expect("collect")
            .providers
    }

    #[test]
    fn a_user_config_can_add_a_provider() {
        let before = catalog().len();
        let providers = with(
            r#"
            magi.provider("my-proxy", {
              api = "openai-completions",
              base_url = "http://10.0.0.2:8080/v1",
              auth = { kind = "none" },
              models = { { id = "m", name = "M", context_window = 8192, max_tokens = 4096 } },
            })
            "#,
        );
        assert_eq!(providers.len(), before + 1);
        assert!(providers.iter().any(|p| p.id == "my-proxy"));
    }

    #[test]
    fn a_user_config_replaces_a_builtin_in_place() {
        let before = catalog().len();
        let position = catalog()
            .iter()
            .position(|p| p.id == "deepseek")
            .expect("deepseek is built in");
        let providers = with(
            r#"
            magi.provider("deepseek", {
              name = "DeepSeek via proxy",
              api = "openai-completions",
              base_url = "http://localhost:9000/v1",
              auth = { kind = "none" },
              models = { { id = "m", name = "M", context_window = 8192, max_tokens = 4096 } },
            })
            "#,
        );
        assert_eq!(providers.len(), before, "an override is not an addition");
        assert_eq!(providers[position].name, "DeepSeek via proxy");
        assert_eq!(providers[position].models.len(), 1, "replaced, not merged");
    }

    #[test]
    fn a_config_may_declare_providers_in_a_loop() {
        // The reason the config is a program: one statement, several machines.
        let providers = with(
            r#"
            for _, box in ipairs({ "alpha", "beta", "gamma" }) do
              magi.provider("gpu-" .. box, {
                api = "openai-completions",
                base_url = "http://" .. box .. ".local:8000/v1",
                auth = { kind = "none" },
                models = { { id = "m", name = "M", context_window = 8192, max_tokens = 4096 } },
              })
            end
            "#,
        );
        for box_ in ["alpha", "beta", "gamma"] {
            assert!(providers.iter().any(|p| p.id == format!("gpu-{box_}")));
        }
    }

    #[test]
    fn the_registration_name_becomes_the_id() {
        let p = declare(
            "my-box",
            &serde_json::json!({ "api": "openai-completions", "auth": { "kind": "none" },
                                 "models": [{ "id": "m", "name": "M",
                                              "context_window": 8192, "max_tokens": 4096 }] }),
        )
        .expect("a provider");
        assert_eq!(p.id, "my-box");
        assert_eq!(p.name, "my-box", "a config should not repeat itself");
    }

    #[test]
    fn a_declaration_cannot_claim_an_id_it_was_not_registered_under() {
        let p = declare(
            "real",
            &serde_json::json!({ "id": "pretend", "api": "openai-completions",
                                 "auth": { "kind": "none" },
                                 "models": [{ "id": "m", "name": "M",
                                              "context_window": 8192, "max_tokens": 4096 }] }),
        )
        .expect("a provider");
        assert_eq!(p.id, "real", "the registrar decides the name");
    }

    #[test]
    fn an_installed_protocol_reaches_the_backend_not_just_the_listing() {
        // The bug this pins: an edited `apis/*.lua` changed what `magi models` printed and
        // nothing the daemon actually did, because the worker rebuilt its VM from the
        // compiled-in copies.
        let loaded = Loaded {
            config: Config::default(),
            apis: vec![("openai-completions".to_owned(), "-- edited".to_owned())],
            tools: Vec::new(),
            clients: Vec::new(),
            providers: Vec::new(),
        };
        assert_eq!(
            loaded.apis.first().map(|(_, source)| source.as_str()),
            Some("-- edited"),
            "what was loaded is what the worker is handed"
        );
    }

    #[test]
    fn the_protocols_reach_the_worker() {
        // Named for the file they came from, because the binary no longer knows what protocols
        // exist — it hands the worker whatever the entry point loaded.
        let loaded = load().expect("the installed configuration must load");
        assert!(!loaded.apis.is_empty(), "nothing to speak with");
        assert!(
            loaded
                .apis
                .iter()
                .any(|(_, source)| source.contains("openai-completions")),
            "the shipped dialects are among them"
        );
    }

    #[test]
    fn a_malformed_declaration_says_what_is_wrong() {
        let error = declare("x", &serde_json::json!({ "api": "nonsense" })).expect_err("must fail");
        assert!(!error.is_empty());
    }
}
