//! Asking a provider what it offers, and remembering the answer.
//!
//! A catalog written by hand goes stale the day it is written. OpenRouter alone offers upwards
//! of four hundred models; `config/providers.lua` listed six, and the six were a generation
//! behind — which reads from the inside as the provider being broken rather than the list being
//! old.
//!
//! **A fetch never touches the configuration.** `providers.lua` is a file somebody wrote and may
//! edit; a program that rewrites it turns every hand-made choice into something that survives
//! until the next refresh. What is fetched goes to a cache under `$XDG_CACHE_HOME/axon/models/`,
//! which nobody edits and anybody may delete.
//!
//! **The cache is the source at load time.** Reading a file is fast and cannot fail because a
//! network is down; a fetch happens when the cache is missing or older than [`FRESH`], and its
//! failure leaves whatever the cache last held.

use axon_provider::model::Model;
use axon_provider::provider::Provider;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// How long a fetched catalog is used before it is asked for again.
const FRESH: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a fetch may take before the cache — even a stale one — is preferred.
///
/// Short on purpose. This runs while somebody is waiting for a prompt, and a catalog that is a
/// day out of date is worth far less than a start that does not hang.
const PATIENCE: Duration = Duration::from_secs(5);

/// What a model is assumed to hold when the provider does not say.
///
/// Wrong for some, and wrong in the safe direction: a window declared smaller than it is costs
/// an early compaction, and one declared larger costs a refused request mid-turn.
const ASSUMED_WINDOW: u64 = 128_000;

/// Fill in the models of every provider that asked to be discovered.
///
/// Providers that declare their models are left alone. A provider that declares neither models
/// nor discovery keeps its empty list, which is what it asked for.
pub fn discover(providers: &mut [Provider]) {
    for provider in providers.iter_mut() {
        if !provider.discover {
            continue;
        }
        let Some(base) = provider.base_url.clone() else {
            continue;
        };
        let found = cached(&provider.id)
            .filter(|held| held.fresh)
            .map(|held| held.models)
            .or_else(|| {
                let key = key_for(provider);
                let fetched = fetch(&base, key.as_deref());
                match fetched {
                    Some(models) if !models.is_empty() => {
                        write_cache(&provider.id, &models);
                        Some(models)
                    }
                    // The network is down, or the key is wrong, or the endpoint is not there.
                    // Yesterday's answer beats no answer.
                    _ => cached(&provider.id).map(|held| held.models),
                }
            });
        if let Some(found) = found {
            provider.models = found
                .into_iter()
                .map(|mut model| {
                    model.provider = provider.id.clone();
                    model.api = provider.api;
                    model
                })
                .collect();
        }
    }
}

/// A cached catalog, and whether it is still worth using without asking again.
struct Held {
    models: Vec<Model>,
    fresh: bool,
}

/// Where a provider's fetched catalog is kept.
fn cache_path(id: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    // Named by the provider id, which a config controls, so it is confined to a single path
    // segment rather than trusted as one.
    let safe: String = id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    Some(
        base.join("axon")
            .join("models")
            .join(format!("{safe}.json")),
    )
}

/// Read what was last fetched for this provider.
fn cached(id: &str) -> Option<Held> {
    let path = cache_path(id)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let models: Vec<Model> = serde_json::from_str(&raw).ok()?;
    let fresh = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|when| SystemTime::now().duration_since(when).ok())
        .is_some_and(|age| age < FRESH);
    Some(Held { models, fresh })
}

/// Keep what was fetched, so the next start does not have to ask.
fn write_cache(id: &str, models: &[Model]) {
    let Some(path) = cache_path(id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(models) {
        let _ = std::fs::write(&path, text);
    }
}

/// The key this provider authenticates with, if the environment has one.
fn key_for(provider: &Provider) -> Option<String> {
    let axon_provider::provider::Auth::ApiKey { vars } = &provider.auth else {
        return None;
    };
    vars.iter().find_map(|name| std::env::var(name).ok())
}

/// Ask `<base>/models` what there is.
///
/// On a thread with a runtime of its own, because this is called from `config::load`, which is
/// synchronous and is reached both from an async daemon and from a plain command. Building a
/// runtime inside a running one panics; building one on a fresh thread cannot.
fn fetch(base: &str, key: Option<&str>) -> Option<Vec<Model>> {
    let url = format!("{}/models", base.trim_end_matches('/'));
    let key = key.map(ToOwned::to_owned);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async move {
            let client = reqwest::Client::builder().timeout(PATIENCE).build().ok()?;
            let mut request = client.get(&url);
            if let Some(key) = key {
                request = request.bearer_auth(key);
            }
            let body: serde_json::Value = request.send().await.ok()?.json().await.ok()?;
            Some(parse(&body))
        })
    })
    .join()
    .ok()
    .flatten()
}

/// Read an OpenAI-shaped `/models` answer into models.
///
/// The shape every provider in this catalog speaks is `{"data": [...]}`. What is *in* an entry
/// varies: OpenRouter carries context, pricing and capabilities; a local Ollama carries an id
/// and little else. Everything but the id is optional, and an absent field takes a default
/// rather than dropping the model — a model you cannot see is worse than one whose price axon
/// does not know.
#[must_use]
pub fn parse(body: &serde_json::Value) -> Vec<Model> {
    let Some(entries) = body.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    entries.iter().filter_map(one).collect()
}

/// One entry of a `/models` answer.
fn one(entry: &serde_json::Value) -> Option<Model> {
    let id = entry.get("id")?.as_str()?.to_owned();
    let name = entry
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(&id)
        .to_owned();
    let window = number(entry, "context_length")
        .or_else(|| {
            entry
                .get("top_provider")
                .and_then(|t| number(t, "context_length"))
        })
        .unwrap_or(ASSUMED_WINDOW);
    let ceiling = entry
        .get("top_provider")
        .and_then(|t| number(t, "max_completion_tokens"))
        .or_else(|| number(entry, "max_completion_tokens"))
        // A quarter of the window, which is what most providers allow and none forbid.
        .unwrap_or((window / 4).max(1));
    Some(Model {
        id,
        name,
        reasoning: reasons(entry),
        context_window: window,
        max_tokens: ceiling,
        cost: cost(entry),
        // Overwritten by the provider, which knows its own id and protocol.
        provider: String::new(),
        api: axon_provider::model::Api::OpenAiCompletions,
        input: vec![axon_provider::model::Modality::Text],
        thinking: std::collections::BTreeMap::new(),
        compat: None,
    })
}

/// Whether the provider says this model can reason.
fn reasons(entry: &serde_json::Value) -> bool {
    entry
        .get("supported_parameters")
        .and_then(|p| p.as_array())
        .is_some_and(|params| {
            params
                .iter()
                .filter_map(|p| p.as_str())
                .any(|p| p == "reasoning" || p == "include_reasoning" || p == "reasoning_effort")
        })
}

/// Price per million tokens, from a per-token price given as a string.
///
/// OpenRouter quotes dollars per token, as a decimal string, and quotes `"0"` for a free model.
/// A price axon cannot read is no price rather than a wrong one.
fn cost(entry: &serde_json::Value) -> axon_model::Cost {
    let Some(pricing) = entry.get("pricing") else {
        return axon_model::Cost::default();
    };
    let each = |name: &str| -> f64 {
        pricing
            .get(name)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .map_or(0.0, |per_token| per_token * 1_000_000.0)
    };
    axon_model::Cost {
        input: each("prompt"),
        output: each("completion"),
        ..axon_model::Cost::default()
    }
}

/// A number that may have arrived as a float, an integer, or a string.
fn number(value: &serde_json::Value, name: &str) -> Option<u64> {
    let field = value.get(name)?;
    field
        .as_u64()
        .or_else(|| field.as_f64().map(|n| n as u64))
        .or_else(|| field.as_str().and_then(|s| s.parse().ok()))
        .filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One entry in the shape OpenRouter answers with.
    fn rich() -> serde_json::Value {
        serde_json::json!({ "data": [{
            "id": "anthropic/claude-opus-5",
            "name": "Claude Opus 5",
            "context_length": 1000000,
            "pricing": { "prompt": "0.000005", "completion": "0.000025" },
            "top_provider": { "max_completion_tokens": 128000 },
            "supported_parameters": ["reasoning", "tools"],
        }]})
    }

    /// What a local Ollama answers with: an id and almost nothing else.
    fn bare() -> serde_json::Value {
        serde_json::json!({ "data": [{ "id": "qwen3:8b" }] })
    }

    #[test]
    fn a_rich_entry_keeps_everything_it_was_told() {
        let found = parse(&rich());
        assert_eq!(found.len(), 1);
        let model = &found[0];
        assert_eq!(model.id, "anthropic/claude-opus-5");
        assert_eq!(model.name, "Claude Opus 5");
        assert_eq!(model.context_window, 1_000_000);
        assert_eq!(model.max_tokens, 128_000);
        assert!(model.reasoning);
    }

    #[test]
    fn a_price_per_token_becomes_a_price_per_million() {
        // OpenRouter quotes dollars per token as a decimal string. A catalog that showed those
        // numbers unchanged would say a model costs five millionths of a cent.
        let model = &parse(&rich())[0];
        assert!(
            (model.cost.input - 5.0).abs() < 1e-9,
            "{}",
            model.cost.input
        );
        assert!(
            (model.cost.output - 25.0).abs() < 1e-9,
            "{}",
            model.cost.output
        );
    }

    #[test]
    fn a_bare_entry_is_kept_rather_than_dropped() {
        // A model you cannot see is worse than one whose price axon does not know.
        let found = parse(&bare());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "qwen3:8b");
        assert_eq!(found[0].name, "qwen3:8b", "the id stands in for a name");
        assert_eq!(found[0].context_window, ASSUMED_WINDOW);
        assert!(!found[0].reasoning);
    }

    #[test]
    fn an_answer_that_is_not_a_catalog_is_no_models_rather_than_a_panic() {
        assert!(parse(&serde_json::json!({ "error": "no" })).is_empty());
        assert!(parse(&serde_json::json!([])).is_empty());
        assert!(parse(&serde_json::json!({ "data": "nonsense" })).is_empty());
    }

    #[test]
    fn an_entry_with_no_id_is_skipped_and_the_rest_survive() {
        let body = serde_json::json!({ "data": [{ "name": "nameless" }, { "id": "real" }] });
        let found = parse(&body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "real");
    }

    #[test]
    fn a_provider_id_cannot_escape_the_cache_directory() {
        // The id comes from a config file, and a config file is a program somebody wrote.
        let path = cache_path("../../etc/passwd").expect("a path");
        assert!(
            path.ends_with("------etc-passwd.json"),
            "{}",
            path.display()
        );
    }

    #[test]
    fn a_declared_catalog_is_never_asked_about() {
        // A provider that lists its models is taken at its word: discovery is opt-in, and a
        // fetch behind somebody's back is a start that hangs when the network is down.
        let mut providers = vec![axon_provider::provider::Provider {
            id: "fixed".into(),
            name: "Fixed".into(),
            base_url: Some("http://127.0.0.1:1/v1".into()),
            api: axon_provider::model::Api::OpenAiCompletions,
            auth: axon_provider::provider::Auth::None,
            compat: None,
            models: Vec::new(),
            discover: false,
        }];
        discover(&mut providers);
        assert!(providers[0].models.is_empty());
    }
}
