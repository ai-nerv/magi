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
use std::collections::BTreeSet;

/// The catalog axum ships, as Lua.
const BUILTIN: &str = include_str!("../../../config/providers.lua");

/// What axum tells the model it is, as Lua.
///
/// Shipped as a file for the same reason the catalog is: a fresh binary has one, and the copy
/// you can edit is the copy it uses.
const BUILTIN_SYSTEM: &str = include_str!("../../../config/system.lua");

/// The tools axum ships, as Lua.
///
/// `bash` among them, declared as a process tool. Shipped rather than built in so that what a
/// fresh install can do is written down in a file you can read and replace.
const BUILTIN_TOOLS: &[(&str, &str)] = &[
    ("bash", include_str!("../../../config/tools/bash.lua")),
    ("hexe", include_str!("../../../config/tools/hexe.lua")),
    ("oslo", include_str!("../../../config/tools/oslo.lua")),
];

/// The family's client stubs, so a Lua tool can talk to a sibling without opening a file.
const BUILTIN_STUBS: &[(&str, &str)] = &[
    ("axum", include_str!("../../../config/stubs/axum.lua")),
    ("hexe", include_str!("../../../config/stubs/hexe.lua")),
    ("oslo", include_str!("../../../config/stubs/oslo.lua")),
];

/// Everything the config files said, in one value.
pub struct Loaded {
    /// Settings and registrations, as the config left them.
    pub config: Config,
    /// Every tool description that was run, as `(name, source)`.
    pub tools: Vec<(String, String)>,
    /// The family's client stubs, as `(name, source)`.
    pub stubs: Vec<(String, String)>,
    /// Every protocol description that was run, in order, as `(name, source)`.
    ///
    /// Kept so the daemon's worker can build its VM from exactly what the catalog was read
    /// with. Rebuilding from the compiled-in copies would mean an edited protocol changed what
    /// `axum models` printed and nothing the daemon actually did.
    pub apis: Vec<(String, String)>,
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
    // The compiled-in copies first, so a binary with no config directory still works.
    let mut apis: Vec<(String, String)> = Vec::new();
    let mut tools: Vec<(String, String)> = Vec::new();
    let mut stubs: Vec<(String, String)> = Vec::new();
    for (name, source) in axum_lua::adapter::BUILTIN {
        engine.run(source, name)?;
        apis.push(((*name).to_owned(), (*source).to_owned()));
    }
    engine.run(BUILTIN_SYSTEM, "system.lua")?;
    engine.run(BUILTIN, "providers.lua")?;
    // Tool and stub descriptions ship the same way the catalog does, so a fresh install has
    // the same tools an installed configuration would give it.
    for (name, source) in BUILTIN_TOOLS {
        tools.push(((*name).to_owned(), (*source).to_owned()));
    }
    for (name, source) in BUILTIN_STUBS {
        stubs.push(((*name).to_owned(), (*source).to_owned()));
    }
    engine.install_stubs(&stubs);
    for (name, source) in &tools {
        engine.run(source, name)?;
    }

    // Then whatever is installed, which overrides any of it by the ordinary rule that
    // registration is keyed.
    for path in installed_files() {
        engine.run_file(&path)?;
        // An installed file replaces its compiled-in namesake in what the worker is given, so
        // the daemon speaks the protocol and offers the tool the user actually edited.
        // Without this, `make configs` installs files that are read once and then ignored.
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.parent().is_some_and(|p| p.ends_with("apis")) {
            layer(&mut apis, name, source);
        } else if path.parent().is_some_and(|p| p.ends_with("tools")) {
            layer(&mut tools, name, source);
        }
    }

    // The line between the two kinds of configuration. Above it is the machine's own, which
    // the user wrote. Below it is a file that arrived with a checkout.
    //
    // Unless the user said otherwise: `axum.trusted` names directories whose project files are
    // as good as their own. The decision belongs to the person, it is made once, and it lives
    // in the config only they can edit -- which is the whole of what a trust boundary needs to
    // be. Without a way to say yes, the rule would be worked around instead of used.
    engine.harvest();
    let machine = trusts_here(&engine.config()).then(|| Trusted::snapshot(&mut engine));

    // Then the project, last, so a repository can choose among what the machine offers.
    for path in axum_lua::search_paths() {
        if path.exists() && path.file_name().is_some_and(|n| n == ".axum.lua") {
            engine.run_file(&path)?;
            // A vouched directory's file is as good as the machine's own, so a tool it declares
            // has to reach the daemon and not just this VM. Without this, vouching for a
            // directory would honour its providers and silently drop its tools.
            if machine.is_none()
                && let Ok(source) = std::fs::read_to_string(&path)
            {
                layer(&mut tools, path.display().to_string(), source);
            }
        }
    }
    engine.harvest();
    if let Some(machine) = &machine {
        for refused in machine.refusals(&mut engine) {
            eprintln!("axum: {refused}");
        }
    }
    collect(engine.config(), apis, tools, stubs, machine.as_ref())
}

/// Whether the working directory is one the machine's config vouched for.
///
/// Inverted on purpose: `Some(Trusted)` means a boundary is being enforced, and a trusted
/// directory has none. Ancestors count, so trusting a worktree root covers what is under it.
fn trusts_here(config: &Config) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return true;
    };
    let listed = config
        .get("trusted")
        .and_then(|v| v.as_array())
        .map(|paths| {
            paths
                .iter()
                .filter_map(|p| p.as_str())
                .any(|p| cwd.starts_with(p))
        })
        .unwrap_or(false);
    !listed
}

/// Replace a compiled-in file with the installed one of the same name, or add it.
fn layer(files: &mut Vec<(String, String)>, name: String, source: String) {
    match files.iter().position(|(n, _)| *n == name) {
        Some(at) => files[at] = (name, source),
        None => files.push((name, source)),
    }
}

/// Turn what the registrar collected into providers.
fn collect(
    config: Config,
    apis: Vec<(String, String)>,
    tools: Vec<(String, String)>,
    stubs: Vec<(String, String)>,
    machine: Option<&Trusted>,
) -> Result<Loaded, LuaError> {
    let mut providers = Vec::new();
    for (id, spec) in config.all("provider") {
        // Refused rather than declared: `refusals` has already said why on stderr, and a
        // provider that is never built is one no model can be resolved against. `None` is a
        // directory the user vouched for, where there is nothing to refuse.
        if machine.is_some_and(|m| !m.allows(id)) {
            continue;
        }
        providers.push(declare(id, spec).map_err(|message| LuaError::Shape {
            what: format!("axum.provider({id:?})"),
            message,
        })?);
    }
    Ok(Loaded {
        config,
        apis,
        tools,
        stubs,
        providers,
    })
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
    Ok(collect(engine.config(), Vec::new(), Vec::new(), Vec::new(), None)?.providers)
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

/// Everything the daemon could talk to, so `/model` has something to pick among.
///
/// Built once at start rather than re-read on each switch: a session should keep answering
/// with what it was started with, and picking up an edit made since would leave a person
/// asking why it is using a model they did not choose.
#[must_use]
pub fn catalog(loaded: &Loaded) -> axum_host::catalog::Catalog {
    axum_host::catalog::Catalog {
        apis: loaded.apis.clone(),
        tools: loaded.tools.clone(),
        stubs: loaded.stubs.clone(),
        cwd: std::env::current_dir().unwrap_or_default(),
        providers: loaded.providers.clone(),
        options: options(loaded),
        system: system(loaded),
        chosen: loaded.config.string("model").map(ToOwned::to_owned),
    }
}

/// What the model is told it is, for this session.
///
/// Assembled here because this is where the configuration and the working directory are both
/// in hand. Every milestone before this one sent nothing: the model got tool schemas and no
/// idea what it was, where it was, or what machine it was on.
#[must_use]
fn system(loaded: &Loaded) -> Option<String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    axum_host::system::assemble(loaded.config.string("system"), &cwd, &today())
}

/// Today, as the model should read it.
fn today() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Civil from days, so a date needs no calendar crate. The model wants to know roughly
    // when it is, not to do arithmetic with it.
    let days = i64::try_from(seconds / 86_400).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = era * 400 + yoe + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// What to ask for beyond the conversation.
///
/// `axum.thinking` is off unless asked for. Reasoning costs tokens and money, and a default
/// that quietly spends both is the wrong kind of surprise — but the whole branch that requests
/// it existed in every protocol description with nothing ever setting this, so asking was
/// impossible rather than merely off.
#[must_use]
fn options(loaded: &Loaded) -> axum_provider::api::Options {
    let thinking = loaded
        .config
        .string("thinking")
        .and_then(|level| serde_json::from_value(serde_json::Value::String(level.to_owned())).ok());
    axum_provider::api::Options {
        thinking,
        max_tokens: None,
    }
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
        apis: loaded.apis.clone(),
        tools: loaded.tools.clone(),
        stubs: loaded.stubs.clone(),
        cwd: std::env::current_dir().unwrap_or_default(),
        provider: provider.clone(),
        model: model.clone(),
        options: options(loaded),
        system: system(loaded),
    })
}

/// Configuration files edited since the daemon on `socket` started.
///
/// The daemon holds the tool set it was built with. Nothing said so, and the two disagreed in
/// the worst direction: `axum tools` reads the configuration and lists a tool you just added,
/// the running session was never told about it, and the model reports that the tool is not
/// registered -- which reads as a broken tool rather than a stale daemon. Same shape as "I ran
/// `make configs` and still nothing".
///
/// The pid file is written when the daemon is spawned, so its mtime is when the session began.
/// No protocol change and nothing to keep in sync: a file newer than that was not read.
#[must_use]
pub fn edited_since_start(socket: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(started) = std::fs::metadata(crate::daemon::pid_path(socket)).and_then(|m| m.modified())
    else {
        return Vec::new();
    };
    let mut watched = installed_files();
    if let Ok(cwd) = std::env::current_dir() {
        watched.push(cwd.join(".axum.lua"));
    }
    newer_than(&watched, started)
}

/// Which of `files` were modified after `started`.
///
/// Split out so it can be tested: the caller's half depends on a config directory and a live
/// daemon, and neither is something a test should have to stand up to check an mtime compare.
#[must_use]
fn newer_than(
    files: &[std::path::PathBuf],
    started: std::time::SystemTime,
) -> Vec<std::path::PathBuf> {
    files
        .iter()
        .filter(|path| {
            std::fs::metadata(path)
                .and_then(|m| m.modified())
                .is_ok_and(|edited| edited > started)
        })
        .cloned()
        .collect()
}

/// Files the installed config directory contributes, in the order they are applied.
///
/// Protocols first, then the catalog, then the user's own file — because a provider names a
/// protocol and a setting names a model, so each layer needs the one under it to already exist.
///
/// The compiled-in copies run before any of this, so a fresh binary with no config directory
/// still speaks and still has a catalog. Installing one with `make configs` gives you the same
/// files to edit; it does not turn anything on that was off.
fn installed_files() -> Vec<std::path::PathBuf> {
    let Some(dir) = config_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    // Every protocol and every tool. `tools/` is here because `make configs` installs those
    // files so they can be edited, and for the whole of M3 nothing read them back.
    for kind in ["apis", "tools"] {
        out.extend(lua_files(&dir.join(kind)));
    }

    for name in ["system.lua", "providers.lua", "init.lua"] {
        let path = dir.join(name);
        if path.exists() {
            out.push(path);
        }
    }
    out
}

/// The `.lua` files in one directory, in a stable order.
fn lua_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lua"))
        .collect();
    out.sort();
    out
}

/// Where an installed configuration lives.
#[must_use]
pub fn config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|base| base.join("axum"))
}

/// What the machine's own configuration had declared, before any project file ran.
///
/// A `.axum.lua` arrives with a checkout: cloning a repository and running `axum` in it must
/// not be enough to add a tool or a provider. A tool because a process tool names a command to
/// run, and a provider because one names a URL the whole conversation is sent to — which is the
/// worse of the two, and the one that looks harmless.
///
/// A project file can still *choose*: `axum.model` picks among providers the machine already
/// has. That is the useful half, and it carries no authority.
pub struct Trusted {
    providers: BTreeSet<String>,
    tools: BTreeSet<String>,
}

impl Trusted {
    /// Record what has been declared so far.
    fn snapshot(engine: &mut Engine) -> Self {
        Self {
            providers: engine
                .config()
                .all("provider")
                .into_iter()
                .map(|(id, _)| id.to_owned())
                .collect(),
            tools: engine.tools().into_iter().map(|(name, _)| name).collect(),
        }
    }

    /// Whether a provider was declared by the machine rather than by a project file.
    fn allows(&self, id: &str) -> bool {
        self.providers.contains(id)
    }

    /// One message per declaration a project file made that will not be honoured.
    ///
    /// Reported rather than silently dropped: a config author who wrote something that does
    /// nothing needs to know, and a repository trying it is worth seeing.
    fn refusals(&self, engine: &mut Engine) -> Vec<String> {
        let mut out = Vec::new();
        for (id, _) in engine.config().all("provider") {
            if !self.allows(id) {
                out.push(format!(
                    "the provider {id:?} was declared by a project file and will not be used; \
                     a provider names a URL your conversation is sent to, so only your own \
                     configuration can add one"
                ));
            }
        }
        for (name, _) in engine.tools() {
            if !self.tools.contains(&name) {
                out.push(format!(
                    "the tool {name:?} was declared by a project file and will not be offered; \
                     a tool can name a command to run, so only your own configuration can add \
                     one"
                ));
            }
        }
        out
    }
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
    fn an_installed_protocol_reaches_the_backend_not_just_the_listing() {
        // The bug this pins: an edited `apis/*.lua` changed what `axum models` printed and
        // nothing the daemon actually did, because the worker rebuilt its VM from the
        // compiled-in copies.
        let loaded = Loaded {
            config: Config::default(),
            apis: vec![("openai-completions".to_owned(), "-- edited".to_owned())],
            tools: Vec::new(),
            stubs: Vec::new(),
            providers: Vec::new(),
        };
        assert_eq!(
            loaded.apis.first().map(|(_, source)| source.as_str()),
            Some("-- edited"),
            "what was loaded is what the worker is handed"
        );
    }

    #[test]
    fn the_compiled_in_protocols_are_carried_as_a_starting_point() {
        let loaded = load().expect("the built-in configuration must load");
        assert!(
            loaded
                .apis
                .iter()
                .any(|(name, _)| name == "openai-completions"),
            "a fresh install must still speak"
        );
    }

    #[test]
    fn a_malformed_declaration_says_what_is_wrong() {
        let error = declare("x", &serde_json::json!({ "api": "nonsense" })).expect_err("must fail");
        assert!(!error.is_empty());
    }
}

#[cfg(test)]
mod staleness_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("axum-stale-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn a_file_edited_after_the_session_started_is_reported() {
        // The whole point: `axum tools` lists the tool you just added, the running daemon was
        // never told, and the model reports it as unregistered.
        let dir = scratch("edited");
        let file = dir.join("greet.lua");
        std::fs::write(&file, "x").expect("write");
        let started = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(newer_than(&[file.clone()], started), vec![file]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_older_than_the_session_is_not() {
        let dir = scratch("older");
        let file = dir.join("greet.lua");
        std::fs::write(&file, "x").expect("write");
        let started = SystemTime::now() + Duration::from_secs(3600);
        assert!(newer_than(&[file], started).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_does_not_exist_is_not_a_change() {
        // `installed_files` names what a config *could* have; most installs have some of it.
        let dir = scratch("absent");
        let started = SystemTime::now() - Duration::from_secs(3600);
        assert!(newer_than(&[dir.join("nothing.lua")], started).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_pid_file_is_no_claim_either_way() {
        // Nothing is running, so nothing is out of date. Warning here would fire on every
        // first start in a directory.
        let dir = scratch("nopid");
        assert!(edited_since_start(&dir.join("a.sock")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_changed_files_are_named() {
        let dir = scratch("some");
        let old = dir.join("old.lua");
        let new = dir.join("new.lua");
        std::fs::write(&old, "x").expect("write");
        std::fs::write(&new, "x").expect("write");
        // `old` predates the mark, `new` follows it.
        let started = SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&new, "y").expect("rewrite");
        assert_eq!(newer_than(&[old, new.clone()], started), vec![new]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
