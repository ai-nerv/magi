//! Loading the config, and the catalog that is part of it.
//!
//! The config is Lua because the interesting configs are programs: probe the machine, loop over
//! a directory of endpoints, branch on whether a GPU box answers. A provider declared in a loop
//! is the same table as one written out by hand, and neither is a fragment anybody has to merge.
//!
//! **The built-in catalog is not special.** It is the first config file, run through the same VM
//! and the same registrar as the user's, so a user file that declares `axon.provider("groq",
//! ...)` replaces it by the ordinary rule that registration is keyed. One mechanism, not two.

use axon_lua::{Config, Engine, LuaError};
use axon_provider::provider::Provider;
use std::collections::BTreeSet;

/// Everything the config files said, in one value.
pub struct Loaded {
    /// Settings and registrations, as the config left them.
    pub config: Config,
    /// Every tool description that was run, as `(name, source)`.
    pub tools: Vec<(String, String)>,
    /// The family's client libraries, as `(name, source)`.
    pub clients: Vec<(String, String)>,
    /// Every protocol description that was run, in order, as `(name, source)`.
    ///
    /// Kept so the daemon's worker can build its VM from exactly what the catalog was read
    /// with. Rebuilding from the compiled-in copies would mean an edited protocol changed what
    /// `axon models` printed and nothing the daemon actually did.
    pub apis: Vec<(String, String)>,
    /// Every provider declared, built-ins first and user files layered over them.
    pub providers: Vec<Provider>,
}

/// Run `init.lua`, then everything it asked for, and collect what they declared.
///
/// **One entry point.** The host runs `init.lua` and nothing else by name; every other file is
/// reached through `axon.load`. Nothing is discovered by scanning, so a file that is not named
/// does not run — the property a plugin mechanism will need, and one a scanner cannot offer.
///
/// **Nothing is compiled in.** A protocol description, a catalog and a tool are configuration:
/// they change without the binary changing, and a binary carrying a copy is a binary you rebuild
/// to fix a wire format. So every one of them is read from the config directory at run time, and
/// `make configs` is what puts them there.
pub fn load() -> Result<Loaded, LuaError> {
    let mut engine = Engine::new();
    let mut apis: Vec<(String, String)> = Vec::new();
    let mut tools: Vec<(String, String)> = Vec::new();
    let mut clients: Vec<(String, String)> = Vec::new();

    let entry = config_dir()
        .map(|dir| dir.join("init.lua"))
        .filter(|path| path.exists())
        .ok_or_else(|| LuaError::Io {
            file: config_dir()
                .map(|d| d.join("init.lua").display().to_string())
                .unwrap_or_else(|| "init.lua".to_owned()),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no configuration; run `make configs` to install it",
            ),
        })?;
    engine.run_file(&entry)?;

    // Drained in rounds so a loaded file may load more, and the clients of a round are installed
    // before its tools run: a tool description opens its sibling's client as it loads.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    loop {
        let asked = engine.take_loads();
        if asked.is_empty() {
            break;
        }
        let mut round: Vec<(String, String)> = Vec::new();
        for path in asked {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(source) = source_of(&path) else {
                continue;
            };
            round.push((path, source));
        }
        for (path, source) in round.iter().filter(|(p, _)| kind(p) == Some("clients")) {
            layer(&mut clients, stem(path), source.clone());
        }
        engine.install_clients(&clients);
        for (path, source) in &round {
            match kind(path) {
                Some("clients") => continue,
                Some("apis") => {
                    engine.run(source, path)?;
                    layer(&mut apis, stem(path), source.clone());
                }
                Some("tools") => {
                    engine.run(source, path)?;
                    layer(&mut tools, stem(path), source.clone());
                }
                _ => engine.run(source, path)?,
            }
        }
    }

    // The line between the two kinds of configuration. Above it is the machine's own, which
    // the user wrote. Below it is a file that arrived with a checkout.
    //
    // Unless the user said otherwise: `axon.trusted` names directories whose project files are
    // as good as their own. The decision belongs to the person, it is made once, and it lives
    // in the config only they can edit -- which is the whole of what a trust boundary needs to
    // be. Without a way to say yes, the rule would be worked around instead of used.
    engine.harvest();
    let machine = trusts_here(&engine.config()).then(|| Trusted::snapshot(&mut engine));

    // Then the project, last, so a repository can choose among what the machine offers.
    for path in axon_lua::search_paths() {
        if path.exists() && path.file_name().is_some_and(|n| n == ".axon.lua") {
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
            eprintln!("axon: {refused}");
        }
    }
    let mut loaded = collect(engine.config(), apis, tools, clients, machine.as_ref())?;
    // Here rather than in `collect`, which `builtin` also uses. `builtin` is the catalog on its
    // own, for when the rest of a config is broken -- a fallback that reaches the network is a
    // fallback that hangs, and it is what the tests read, which would make them ask the internet
    // what they are asserting about.
    catalog::discover(&mut loaded.providers);
    Ok(loaded)
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
    clients: Vec<(String, String)>,
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
            what: format!("axon.provider({id:?})"),
            message,
        })?);
    }
    Ok(Loaded {
        config,
        apis,
        tools,
        clients,
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

/// The catalog alone, for when the rest of a config is broken or irrelevant.
pub fn builtin() -> Result<Vec<Provider>, LuaError> {
    let mut engine = Engine::new();
    if let Some(source) = source_of("providers.lua") {
        engine.run(&source, "providers.lua")?;
    }
    engine.harvest();
    Ok(collect(engine.config(), Vec::new(), Vec::new(), Vec::new(), None)?.providers)
}

/// Find the model the config chose, and the provider offering it.
///
/// `provider/model`, as `axon models` prints it. A bare model id is matched too, because a
/// person who has one provider configured should not have to say which — but an ambiguous bare
/// id resolves to the first declared, which is why the qualified form is what gets printed.
#[must_use]
pub fn resolve<'a>(
    providers: &'a [Provider],
    name: &str,
) -> Option<(&'a Provider, &'a axon_provider::model::Model)> {
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

/// Everything the daemon could talk to, so `:model` has something to pick among.
///
/// Built once at start rather than re-read on each switch: a session should keep answering
/// with what it was started with, and picking up an edit made since would leave a person
/// asking why it is using a model they did not choose.
#[must_use]
pub fn catalog(loaded: &Loaded) -> axon_host::catalog::Catalog {
    axon_host::catalog::Catalog {
        apis: loaded.apis.clone(),
        tools: loaded.tools.clone(),
        clients: loaded.clients.clone(),
        cwd: std::env::current_dir().unwrap_or_default(),
        providers: loaded.providers.clone(),
        options: options(loaded),
        system: system(loaded),
        grants: grants(loaded),
        environ: environ(loaded),
        idle_exit: idle_exit(loaded),
        chosen: asked(loaded),
        confine: loaded.config.boolean("confine").unwrap_or(false),
    }
}

pub(crate) mod chosen;
use chosen::{asked, chosen};
mod catalog;
mod settings;

use settings::{grants, idle_exit, options, system};

pub use settings::{adopt_ui, environ};

/// What this directory chose last time it was used.
#[must_use]
pub fn remembered() -> axon_host::remember::Chosen {
    std::env::current_dir()
        .map(|cwd| axon_host::remember::of(&cwd.display().to_string()))
        .unwrap_or_default()
}

/// The backend a daemon should run turns against, if one is both chosen and usable.
///
/// A model that is configured but has no credential yields `None` rather than an error: the
/// daemon still starts, and the refusal it journals names what to set. A daemon that would not
/// start because a key was missing is a worse answer than a session that says so.
#[must_use]
pub fn backend(loaded: &Loaded) -> Option<axon_host::turn::Backend> {
    let (provider, model) = chosen(loaded)?;
    Some(axon_host::turn::Backend {
        apis: loaded.apis.clone(),
        tools: loaded.tools.clone(),
        clients: loaded.clients.clone(),
        cwd: std::env::current_dir().unwrap_or_default(),
        provider: provider.clone(),
        model: model.clone(),
        options: options(loaded),
        system: system(loaded),
        grants: grants(loaded),
        environ: environ(loaded),
        confine: loaded.config.boolean("confine").unwrap_or(false),
    })
}

/// Configuration files edited since the daemon on `socket` started.
///
/// The daemon holds the tool set it was built with. Nothing said so, and the two disagreed in
/// the worst direction: `axon tools` reads the configuration and lists a tool you just added,
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
    let mut watched = watched_files();
    if let Ok(cwd) = std::env::current_dir() {
        watched.push(cwd.join(".axon.lua"));
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
/// Every installed file a session could have read, for the staleness check.
///
/// Not what gets loaded — `init.lua` decides that, and only what it names runs. This is the
/// wider net a "your config changed since this daemon started" warning wants: a file the user
/// edited is worth mentioning whether or not their entry point currently reaches it.
fn watched_files() -> Vec<std::path::PathBuf> {
    let Some(dir) = config_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    for group in ["apis", "tools", "clients"] {
        out.extend(lua_files(&dir.join(group)));
    }

    for name in ["apis.lua", "tools.lua", "providers.lua", "init.lua"] {
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
        .map(|base| base.join("axon"))
}

/// What the machine's own configuration had declared, before any project file ran.
///
/// A `.axon.lua` arrives with a checkout: cloning a repository and running `axon` in it must
/// not be enough to add a tool or a provider. A tool because a process tool names a command to
/// run, and a provider because one names a URL the whole conversation is sent to — which is the
/// worse of the two, and the one that looks harmless.
///
/// A project file can still *choose*: `axon.model` picks among providers the machine already
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

/// The source behind one `axon.load` path, read from the config directory.
///
/// A path that is not there is skipped rather than fatal: an entry point may load a file that is
/// optional on this machine, and a missing optional is not a broken configuration.
fn source_of(path: &str) -> Option<String> {
    let file = config_dir()?.join(path);
    std::fs::read_to_string(file).ok()
}

/// The name a loaded file registers under: its stem, without directory or extension.
fn stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// Which bucket a loaded path belongs to, if any.
///
/// By the first path component, so `apis.lua` and `apis/google.lua` land in the same place: the
/// shipped tree keeps one file per kind, and somebody who prefers a file per protocol should not
/// have to tell the host about it.
fn kind(path: &str) -> Option<&'static str> {
    for name in ["apis", "tools", "clients"] {
        if path == format!("{name}.lua") || path.starts_with(&format!("{name}/")) {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod catalog_tests;

#[cfg(test)]
mod staleness_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("axon-stale-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn a_file_edited_after_the_session_started_is_reported() {
        // The whole point: `axon tools` lists the tool you just added, the running daemon was
        // never told, and the model reports it as unregistered.
        let dir = scratch("edited");
        let file = dir.join("greet.lua");
        std::fs::write(&file, "x").expect("write");
        let started = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(newer_than(std::slice::from_ref(&file), started), vec![file]);
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
        // `watched_files` names what a config *could* have; most installs have some of it.
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
