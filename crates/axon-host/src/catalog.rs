//! Every model this daemon could talk to, and how to reach each one.
//!
//! A [`crate::turn::Backend`] names one model. Switching to another needs the parts a
//! backend does *not* vary — the protocol descriptions, the tools, the clients, the working
//! directory — kept somewhere that outlives the choice. That is this.
//!
//! Held by the daemon rather than re-read from configuration on each switch, so `/model` picks
//! among what this session actually started with. Re-reading would mean a switch could silently
//! pick up an edit made since, and "why is it using a model I did not choose" is a bad question
//! to be left with.

use crate::turn::Backend;
use axon_provider::model::Model;
use axon_provider::provider::Provider;

/// The models a session can choose between.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// Protocol descriptions, as `(name, source)`.
    pub apis: Vec<(String, String)>,
    /// Tool descriptions, as `(name, source)`.
    pub tools: Vec<(String, String)>,
    /// The family's client libraries, as `(name, source)`.
    pub clients: Vec<(String, String)>,
    /// Where the session is rooted.
    pub cwd: std::path::PathBuf,
    /// Every provider that was declared.
    pub providers: Vec<Provider>,
    /// What to ask for beyond the conversation.
    pub options: axon_provider::api::Options,
    /// What the model is told it is.
    pub system: Option<String>,
    /// Whether the file tools refuse paths outside `cwd`.
    pub confine: bool,
    /// Permissions a configuration granted before anybody was asked anything.
    pub grants: Vec<axon_proto::permit::Grant>,
    /// Environment every process this session starts is given, beside the mandatory pairs.
    pub environ: std::collections::BTreeMap<String, String>,
    /// The model the configuration asked for, whether or not it can be reached.
    ///
    /// Kept so a refusal can name it. Without this the daemon could only say "no model
    /// is configured", which is false whenever one is configured and merely unusable.
    pub chosen: Option<String>,
}

impl Catalog {
    /// A catalog offering nothing.
    ///
    /// For a daemon started without one: `/model` then has nothing to switch to and says so,
    /// which is the truth rather than a crash.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            apis: Vec::new(),
            tools: Vec::new(),
            clients: Vec::new(),
            environ: std::collections::BTreeMap::new(),
            cwd: std::path::PathBuf::new(),
            providers: Vec::new(),
            options: axon_provider::api::Options::default(),
            system: None,
            chosen: None,
            confine: false,
            grants: Vec::new(),
        }
    }

    /// The backend for a model, by qualified or bare name.
    ///
    /// `None` when no such model exists, or when its provider has no credential. The second is
    /// not an error: the model is real and the answer is to set a key, which is what
    /// [`Self::unusable`] explains.
    #[must_use]
    pub fn backend(&self, name: &str) -> Option<Backend> {
        let (provider, model) = self.find(name)?;
        provider.is_configured().then(|| Backend {
            apis: self.apis.clone(),
            tools: self.tools.clone(),
            clients: self.clients.clone(),
            environ: self.environ.clone(),
            cwd: self.cwd.clone(),
            provider: provider.clone(),
            model: model.clone(),
            options: self.options.clone(),
            system: self.system.clone(),
            confine: self.confine,
            grants: self.grants.clone(),
        })
    }

    /// Why a named model cannot be used, when that is the reason it was refused.
    ///
    /// Separated from `backend` because "there is no such model" and "you have not set a key
    /// for it" send a person to two different places, and a single `None` sends them to
    /// neither.
    #[must_use]
    pub fn unusable(&self, name: &str) -> Option<String> {
        let (provider, _) = self.find(name)?;
        (!provider.is_configured()).then(|| {
            format!(
                "{name} is offered by {}, which is not configured: {}",
                provider.name,
                provider.auth.requirement()
            )
        })
    }

    /// Every model in the catalog, ready or not, with what it would take.
    ///
    /// All of them, because the person asking has usually configured nothing: a list of the
    /// two local providers they do not run teaches less than a list of forty with "set
    /// ANTHROPIC_API_KEY" beside the one they wanted.
    #[must_use]
    pub fn choices(&self) -> Vec<axon_proto::ModelChoice> {
        let mut out: Vec<axon_proto::ModelChoice> = self
            .providers
            .iter()
            .flat_map(|provider| {
                let requirement = if provider.is_configured() {
                    String::new()
                } else {
                    provider.auth.requirement()
                };
                let wants: Vec<String> = if provider.is_configured() {
                    Vec::new()
                } else {
                    provider.auth.vars().to_vec()
                };
                provider
                    .models
                    .iter()
                    .map(move |model| axon_proto::ModelChoice {
                        name: model.qualified(),
                        context_window: model.context_window,
                        requirement: requirement.clone(),
                        wants_vars: wants.clone(),
                        reasoning: model.reasoning,
                    })
            })
            .collect();
        // Ready ones first, then by name: the list is for choosing from, and what you can
        // choose right now belongs at the top of it.
        out.sort_by(|a, b| {
            a.requirement
                .is_empty()
                .cmp(&b.requirement.is_empty())
                .reverse()
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    /// Every model that could be switched to right now, qualified and sorted.
    ///
    /// Only the configured ones. A list that includes forty models you cannot reach is a list
    /// nobody reads to the end of.
    #[must_use]
    pub fn usable(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .providers
            .iter()
            .filter(|p| p.is_configured())
            .flat_map(|p| p.models.iter().map(Model::qualified))
            .collect();
        out.sort();
        out
    }

    /// Resolve a name the way the configuration does.
    fn find(&self, name: &str) -> Option<(&Provider, &Model)> {
        if let Some((provider_id, model_id)) = name.split_once('/')
            && let Some(provider) = self.providers.iter().find(|p| p.id == provider_id)
            && let Some(model) = provider.model(model_id)
        {
            // Split at the first slash only: several catalogs use slashes inside a model id,
            // so `openrouter/anthropic/claude-sonnet-4.5` is one provider and one model.
            return Some((provider, model));
        }
        self.providers
            .iter()
            .find_map(|p| p.model(name).map(|m| (p, m)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_provider::provider::Auth;

    fn catalog() -> Catalog {
        let providers = serde_json::from_value::<Vec<Provider>>(serde_json::json!([
            {
                "id": "local", "name": "Local", "api": "openai-completions",
                "base_url": "http://localhost:1234/v1", "auth": { "kind": "none" },
                "models": [
                    { "id": "a", "name": "A", "context_window": 1000, "max_tokens": 100 },
                    { "id": "b", "name": "B", "context_window": 1000, "max_tokens": 100 }
                ]
            },
            {
                "id": "paid", "name": "Paid Co", "api": "openai-completions",
                "base_url": "https://paid.test/v1",
                "auth": { "kind": "api-key", "vars": ["AXON_TEST_NOT_SET"] },
                "models": [
                    { "id": "x", "name": "X", "context_window": 1000, "max_tokens": 100 }
                ]
            }
        ]))
        .expect("providers");
        Catalog {
            apis: Vec::new(),
            tools: Vec::new(),
            clients: Vec::new(),
            environ: std::collections::BTreeMap::new(),
            cwd: std::env::temp_dir(),
            providers,
            options: axon_provider::api::Options::default(),
            system: None,
            chosen: None,
            confine: false,
            grants: Vec::new(),
        }
    }

    #[test]
    fn a_configured_model_yields_a_backend() {
        let backend = catalog().backend("local/a").expect("a backend");
        assert_eq!(backend.model.id, "a");
        assert_eq!(backend.provider.id, "local");
    }

    #[test]
    fn a_bare_name_resolves_when_it_is_unambiguous() {
        assert_eq!(catalog().backend("b").expect("a backend").model.id, "b");
    }

    #[test]
    fn a_model_id_containing_a_slash_is_still_one_model() {
        // `openrouter/anthropic/claude-sonnet-4.5` is one provider and one model, and
        // splitting on every slash makes it neither.
        let mut catalog = catalog();
        catalog.providers.push(
            serde_json::from_value(serde_json::json!({
                "id": "router", "name": "Router", "api": "openai-completions",
                "base_url": "https://r.test/v1", "auth": { "kind": "none" },
                "models": [{ "id": "vendor/m", "name": "M",
                             "context_window": 1000, "max_tokens": 100 }]
            }))
            .expect("provider"),
        );
        let backend = catalog.backend("router/vendor/m").expect("a backend");
        assert_eq!(backend.model.id, "vendor/m");
    }

    #[test]
    fn a_model_with_no_credential_is_refused_with_a_reason() {
        // "No such model" and "you have not set a key" send a person to two different places.
        let catalog = catalog();
        assert!(catalog.backend("paid/x").is_none());
        let why = catalog.unusable("paid/x").expect("a reason");
        assert!(why.contains("AXON_TEST_NOT_SET"), "{why}");
    }

    #[test]
    fn a_model_that_does_not_exist_has_no_reason_to_give() {
        assert!(catalog().unusable("nope/nope").is_none());
    }

    #[test]
    fn only_reachable_models_are_offered() {
        // A list of forty models you cannot use is a list nobody reads to the end of.
        let usable = catalog().usable();
        assert_eq!(usable, vec!["local/a".to_owned(), "local/b".to_owned()]);
    }

    #[test]
    fn a_provider_needing_no_credential_counts_as_configured() {
        assert!(matches!(catalog().providers[0].auth, Auth::None));
        assert!(catalog().backend("local/a").is_some());
    }
}

impl Catalog {
    /// The model the configuration asked for.
    #[must_use]
    pub fn chosen(&self) -> Option<String> {
        self.chosen.clone()
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::*;

    #[test]
    fn a_catalog_remembers_what_was_asked_for() {
        // Without this the daemon could only say "no model is configured", which is false
        // whenever one is configured and merely unusable — the common case for somebody whose
        // environment holds a key for a different provider than the one their config names.
        let mut catalog = Catalog::empty();
        catalog.chosen = Some("anthropic/claude-sonnet-4-5".into());
        assert_eq!(
            catalog.chosen().as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
    }

    #[test]
    fn an_empty_catalog_asked_for_nothing() {
        assert!(Catalog::empty().chosen().is_none());
    }
}
