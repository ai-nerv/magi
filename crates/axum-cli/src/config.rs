//! Loading the config, and the catalog that is part of it.
//!
//! The config is Lua because the interesting configs are programs: probe the machine, loop over
//! a directory of endpoints, branch on whether a GPU box answers. A provider declared in a loop
//! is the same table as one written out by hand, and neither is a fragment anybody has to merge.
//!
//! **The built-in catalog is not special.** It is the first config file, run through the same VM
//! and the same registrar as the user's, so a user file that declares `axum.provider("groq",
//! ...)` replaces it by the ordinary rule that registration is keyed. One mechanism, not two.

use axum_lua::{Config, Engine, LuaError};
use axum_provider::provider::Provider;

/// The catalog axum ships, as Lua.
const BUILTIN: &str = include_str!("../lua/providers.lua");

/// Everything the config files said, in one value.
pub struct Loaded {
    /// Settings and registrations, as the config left them.
    pub config: Config,
    /// Every provider declared, built-ins first and user files layered over them.
    pub providers: Vec<Provider>,
}

/// Run the built-in catalog, then every config file, and collect what they declared.
///
/// A missing user file is not an error: most people have no config, and the ones who do should
/// not have to create an empty one in every project. A file that *exists* and does not load is
/// fatal, because it expressed an intention that has not been carried out.
pub fn load() -> Result<Loaded, LuaError> {
    let mut engine = Engine::new();
    engine.run(BUILTIN, "providers.lua")?;
    for path in axum_lua::search_paths() {
        if path.exists() {
            engine.run_file(&path)?;
        }
    }
    engine.harvest();
    collect(engine.config())
}

/// Turn what the registrar collected into providers.
fn collect(config: Config) -> Result<Loaded, LuaError> {
    let mut providers = Vec::new();
    for (id, spec) in config.all("provider") {
        providers.push(declare(id, spec).map_err(|message| LuaError::Shape {
            what: format!("axum.provider({id:?})"),
            message,
        })?);
    }
    Ok(Loaded { config, providers })
}

/// Build a provider from what the config handed the registrar.
///
/// The id comes from the registration rather than the table, so a config cannot declare one
/// name and register another — and a loop over a directory names each entry by its file.
fn declare(id: &str, spec: &serde_json::Value) -> Result<Provider, String> {
    let mut object = spec.as_object().cloned().unwrap_or_default();
    object.insert("id".into(), serde_json::Value::String(id.to_owned()));
    object
        .entry("name")
        .or_insert_with(|| serde_json::Value::String(id.to_owned()));
    serde_json::from_value(serde_json::Value::Object(object)).map_err(|e| e.to_string())
}

/// The built-in catalog alone, for when a user config is broken or irrelevant.
pub fn builtin() -> Result<Vec<Provider>, LuaError> {
    let mut engine = Engine::new();
    engine.run(BUILTIN, "providers.lua")?;
    engine.harvest();
    Ok(collect(engine.config())?.providers)
}

/// Find the model the config chose, and the provider offering it.
///
/// `provider/model`, as `axum models` prints it. A bare model id is matched too, because a
/// person who has one provider configured should not have to say which — but an ambiguous bare
/// id resolves to the first declared, which is why the qualified form is what gets printed.
#[must_use]
pub fn resolve<'a>(
    providers: &'a [Provider],
    name: &str,
) -> Option<(&'a Provider, &'a axum_provider::model::Model)> {
    if let Some((provider_id, model_id)) = name.split_once('/') {
        // Split at the first slash only: several catalogs use slashes inside a model id, so
        // `openrouter/anthropic/claude-sonnet-4.5` is one provider and one model.
        if let Some(provider) = providers.iter().find(|p| p.id == provider_id)
            && let Some(model) = provider.model(model_id)
        {
            return Some((provider, model));
        }
    }
    providers.iter().find_map(|p| p.model(name).map(|m| (p, m)))
}

/// The backend a daemon should run turns against, if one is both chosen and usable.
///
/// A model that is configured but has no credential yields `None` rather than an error: the
/// daemon still starts, and the refusal it journals names what to set. A daemon that would not
/// start because a key was missing is a worse answer than a session that says so.
#[must_use]
pub fn backend(loaded: &Loaded) -> Option<axum_host::turn::Backend> {
    let name = loaded.config.string("model")?;
    let (provider, model) = resolve(&loaded.providers, name)?;
    if !provider.is_configured() {
        return None;
    }
    Some(axum_host::turn::Backend {
        provider: provider.clone(),
        model: model.clone(),
        options: axum_provider::api::Options::default(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum_provider::model::Api;
    use std::collections::BTreeSet;

    fn catalog() -> Vec<Provider> {
        builtin().expect("the built-in catalog must load")
    }

    #[test]
    fn the_builtin_catalog_is_a_config_file_that_runs() {
        assert!(catalog().len() >= 40, "only {}", catalog().len());
    }

    #[test]
    fn the_catalog_covers_every_protocol() {
        let apis: BTreeSet<Api> = catalog().iter().map(|p| p.api).collect();
        for api in Api::all() {
            assert!(apis.contains(&api), "nothing speaks {}", api.as_str());
        }
    }

    #[test]
    fn most_providers_share_one_adapter() {
        let providers = catalog();
        let shared = providers
            .iter()
            .filter(|p| p.api == Api::OpenAiCompletions)
            .count();
        assert!(
            shared * 2 > providers.len(),
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
    fn every_provider_offers_at_least_one_model() {
        for p in catalog() {
            assert!(!p.models.is_empty(), "{} offers nothing", p.id);
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
        engine.run(BUILTIN, "providers.lua").expect("builtin");
        engine.run(extra, "user.lua").expect("user config");
        engine.harvest();
        collect(engine.config()).expect("collect").providers
    }

    #[test]
    fn a_user_config_can_add_a_provider() {
        let before = catalog().len();
        let providers = with(
            r#"
            axum.provider("my-proxy", {
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
            .position(|p| p.id == "groq")
            .expect("groq is built in");
        let providers = with(
            r#"
            axum.provider("groq", {
              name = "Groq via proxy",
              api = "openai-completions",
              base_url = "http://localhost:9000/v1",
              auth = { kind = "none" },
              models = { { id = "m", name = "M", context_window = 8192, max_tokens = 4096 } },
            })
            "#,
        );
        assert_eq!(providers.len(), before, "an override is not an addition");
        assert_eq!(providers[position].name, "Groq via proxy");
        assert_eq!(providers[position].models.len(), 1, "replaced, not merged");
    }

    #[test]
    fn a_config_may_declare_providers_in_a_loop() {
        // The reason the config is a program: one statement, several machines.
        let providers = with(
            r#"
            for _, box in ipairs({ "alpha", "beta", "gamma" }) do
              axum.provider("gpu-" .. box, {
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
    fn a_malformed_declaration_says_what_is_wrong() {
        let error = declare("x", &serde_json::json!({ "api": "nonsense" })).expect_err("must fail");
        assert!(!error.is_empty());
    }
}
