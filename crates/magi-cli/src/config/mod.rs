//! Loading the config, and the catalog that is part of it. The config is Lua because the interesting
//! configs are programs. The built-in catalog is the first config file, run through the same VM and
//! the same registrar as the user's, so a user file declaring the same name replaces it.

use magi_lua::{Config, Engine, LuaError};
use std::collections::BTreeSet;

/// Everything the config files said, in one value.
pub struct Loaded {
    pub config: Config,
    pub tools: Vec<(String, String)>,
    pub clients: Vec<(String, String)>,
}

/// Run `init.lua`, then everything it asked for, and collect what they declared. `init.lua` is the
/// only file run by name; what is installed is discovered — see [`discovered`]. Nothing is compiled
/// in: protocol descriptions, catalogs and tools are read from the config directory at run time.
pub fn load() -> Result<Loaded, LuaError> {
    let mut engine = Engine::new();
    let mut tools: Vec<(String, String)> = Vec::new();

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

    // Read before anything is borrowed and before any tool description runs: a role names the
    // program magi asks for a client library, and a tool description opens that library as it
    // loads. Said to the VM as well as to this process, since the VM reading the configuration is
    // the one that learned the roles and is already running by the time they are known.
    let filled = roles::said(&mut engine);
    magi_lua::name_roles(&filled);
    engine.install_roles(&filled);
    // Every sibling serves its own client library, so magi asks rather than vendoring — a stale
    // copy silently removed every memory tool from every session. See `lent`.
    let mut clients: Vec<(String, String)> = lent::borrowed(&roles::programs(&filled));
    engine.install_clients(&clients);

    // Drained in rounds so a loaded file may load more; a round's clients are installed before its
    // tools run, since a tool description opens its sibling's client as it loads.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut drain = |engine: &mut Engine| -> Result<(), LuaError> {
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
                    // Run like any other file, but nothing is kept: a protocol description is
                    // melchior's now.
                    Some("apis") => engine.run(source, path)?,
                    Some("tools") => {
                        engine.run(source, path)?;
                        layer(&mut tools, stem(path), source.clone());
                    }
                    _ => engine.run(source, path)?,
                }
            }
        }
        Ok(())
    };
    drain(&mut engine)?;

    // Then whatever is installed, after everything `init.lua` named and before the project file.
    let cwd = std::env::current_dir().unwrap_or_default();
    let found = discovered::run(
        &mut engine,
        &magi_lua::plugins::Roots::discovered(&cwd),
        &mut drain,
    )?;
    // Kept because the session rebuilds its VM from these on the worker's thread — a Lua state does
    // not cross a thread — and appended in runtimepath order, so `after/plugin/` still means last.
    for (name, source) in found {
        layer(&mut tools, name, source);
    }

    // The line between the machine's own configuration and a file that arrived with a checkout,
    // unless `magi.trusted` names the directory: the decision lives in the config only the user edits.
    engine.harvest();
    let machine = trusts_here(&engine.config()).then(|| Trusted::snapshot(&mut engine));

    // Then the project, last, so a repository can choose among what the machine offers.
    for path in magi_lua::search_paths() {
        if path.exists() && path.file_name().is_some_and(|n| n == ".magi.lua") {
            engine.run_file(&path)?;
            // A vouched directory's file is as good as the machine's own, so its tools have to
            // reach the daemon and not just this VM.
            if machine.is_none()
                && let Ok(source) = std::fs::read_to_string(&path)
            {
                layer(&mut tools, path.display().to_string(), source);
            }
        }
    }
    engine.harvest();

    // Everything a config said that magi did not keep, whoever said it; both cases were silent before.
    for said in &engine.config().unkept {
        eprintln!("magi: {said}");
    }

    if let Some(machine) = &machine {
        // A changed privileged setting is fatal: a project file has already changed how the rest of
        // the session is governed, and the value it wanted is the value the config now holds.
        if let Some(message) = machine.altered(&mut engine) {
            return Err(LuaError::Runtime {
                file: ".magi.lua".to_owned(),
                message,
            });
        }
        for refused in machine.refusals(&mut engine) {
            eprintln!("magi: {refused}");
        }
    }
    collect(engine.config(), tools, clients)
}

/// Acknowledge every installed package, so it may run. Nothing installed is not an error.
pub fn acknowledge(how: crate::verbs::As) {
    let refuse = |why: &str| {
        if how.framed() {
            crate::verbs::say(&magi_ipc::family::Reply::refused(why), how);
        } else {
            eprintln!("magi: {why}");
        }
    };
    let Some(dir) = config_dir() else {
        refuse("no configuration directory to write a manifest in");
        return;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let files = discovered::installed(&magi_lua::plugins::Roots::discovered(&cwd));
    let manifest = magi_lua::acknowledged::manifest_in(&dir);

    // Written even when there is nothing, because the manifest replaces rather than merges: a
    // digest left behind would still acknowledge a package that was removed.
    match magi_lua::acknowledged::acknowledge(&manifest, &files) {
        Ok(taken) => {
            if how.framed() {
                let rows = files
                    .iter()
                    .map(|(path, _)| serde_json::json!({ "file": path.display().to_string() }))
                    .collect();
                crate::verbs::say(&magi_ipc::family::Reply::rows(rows), how);
                return;
            }
            for (path, _) in &files {
                println!("  {}", path.display());
            }
            match taken {
                0 => println!("nothing installed under site/pack — the manifest is now empty"),
                n => println!("acknowledged {n} file(s) in {}", manifest.display()),
            }
        }
        Err(why) => refuse(&why.to_string()),
    }
}
/// Whether the working directory is one the machine's config vouched for. Inverted on purpose:
/// `Some(Trusted)` means a boundary is enforced. Ancestors count, so a worktree root covers what is under it.
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

/// Everything the registrar collected, as one value. No providers: melchior owns the model.
fn collect(
    config: Config,
    tools: Vec<(String, String)>,
    clients: Vec<(String, String)>,
) -> Result<Loaded, LuaError> {
    Ok(Loaded {
        config,
        tools,
        clients,
    })
}

/// Which program owns the model here: `magi.melchior` when a configuration named one, and the
/// sibling's own name otherwise. One function, so the layer and the model list cannot disagree.
#[must_use]
pub fn mind(loaded: &Loaded) -> String {
    roles::fills(loaded, "model")
}

/// Which program holds this session's history — the `memory` role, as `magi.memory` named it.
#[must_use]
pub fn memory(loaded: &Loaded) -> String {
    roles::fills(loaded, "memory")
}

/// Everything the daemon could talk to, so `:model` has something to pick among. Built once at start
/// rather than re-read on each switch. The cards come from melchior, which owns them.
#[must_use]
pub fn catalog(loaded: &Loaded, cards: Vec<magi_proto::ask::Card>) -> magi_host::catalog::Catalog {
    let mut catalog = magi_host::catalog::Catalog {
        mind: mind(loaded),
        memory: memory(loaded),
        tooling: tooling(loaded),
        tools: loaded.tools.clone(),
        clients: loaded.clients.clone(),
        cwd: std::env::current_dir().unwrap_or_default(),
        cards,
        wants: options(loaded),
        system: system(loaded),
        grants: grants(loaded),
        environ: environ(loaded),
        chosen: None,
        confine: loaded.config.boolean("confine").unwrap_or(false),
    };
    // After the cards: resolving what was asked for needs something to resolve it against.
    catalog.chosen = asked(loaded, &catalog);
    catalog
}

pub(crate) mod chosen;
mod discovered;
mod lent;
pub mod roles;
use chosen::asked;
mod settings;

use settings::{grants, options, system};

pub use settings::{adopt_ui, environ, grants as granted, tooling};

/// What this directory chose last time it was used.
#[must_use]
pub fn remembered() -> magi_host::remember::Chosen {
    std::env::current_dir()
        .map(|cwd| magi_host::remember::of(&cwd.display().to_string()))
        .unwrap_or_default()
}

/// The backend a daemon should run turns against, if one is both chosen and usable. A model that is
/// configured but has no credential yields `None`, so the daemon still starts and journals a refusal.
#[must_use]
pub fn backend(catalog: &magi_host::catalog::Catalog) -> Option<magi_host::turn::Backend> {
    catalog
        .chosen()
        .as_deref()
        .and_then(|name| catalog.backend(name))
}

/// Configuration files edited since the daemon on `socket` started. A session holds the tool set it
/// was built with, and the socket's mtime is when the session began, so a newer file was not read.
#[must_use]
pub fn edited_since_start(socket: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(started) = std::fs::metadata(socket).and_then(|m| m.modified()) else {
        return Vec::new();
    };
    let mut watched = watched_files();
    if let Ok(cwd) = std::env::current_dir() {
        watched.push(cwd.join(".magi.lua"));
    }
    newer_than(&watched, started)
}

/// Which of `files` were modified after `started`. Split out so it can be tested without a daemon.
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

/// Files the installed config directory contributes, in the order they are applied. Not what gets
/// loaded — `init.lua` decides that — but the wider net a staleness warning wants.
fn watched_files() -> Vec<std::path::PathBuf> {
    let Some(dir) = config_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    for group in ["apis", "tools", "clients"] {
        out.extend(lua_files(&dir.join(group)));
    }

    // The discovered roots too: a plugin is configuration like any other.
    for group in ["plugin", "after/plugin"] {
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
        .map(|base| base.join("magi"))
}

/// What the machine's own configuration had declared, before any project file ran. A `.magi.lua`
/// arrives with a checkout, and cloning a repository must not be enough to add a tool. A project
/// file can still *choose* with `magi.model`, which carries no authority.
pub struct Trusted {
    tools: BTreeSet<String>,
    /// What [`PRIVILEGED_SETTINGS`] were before a project file ran.
    settings: Vec<Option<serde_json::Value>>,
    /// Which program filled each role before a project file ran. Held apart from the settings
    /// because what is privileged is the *name*: `magi.melchior = { … }` is a settings table a
    /// project may write, and `magi.melchior = "./x"` is a program it may not.
    roles: Vec<(String, String)>,
}

/// Settings a project's own file may not assign: `confine` is the wall, `allow` is what may happen
/// without asking, and a file that could set `trusted` could exempt itself.
const PRIVILEGED_SETTINGS: &[&str] = &["confine", "allow", "trusted"];

impl Trusted {
    /// Record what has been declared so far.
    fn snapshot(engine: &mut Engine) -> Self {
        Self {
            tools: engine.tools().into_iter().map(|(name, _)| name).collect(),
            settings: Self::privileged(engine),
            roles: roles::said(engine),
        }
    }

    /// The privileged settings as they stand, in `PRIVILEGED_SETTINGS` order.
    fn privileged(engine: &mut Engine) -> Vec<Option<serde_json::Value>> {
        let config = engine.config();
        PRIVILEGED_SETTINGS
            .iter()
            .map(|name| config.get(name).cloned())
            .collect()
    }

    /// Why the session may not start, if a project file changed something privileged. Compared by
    /// value, so a project file that reads `magi.confine` and assigns it back has changed nothing.
    fn altered(&self, engine: &mut Engine) -> Option<String> {
        let now = Self::privileged(engine);
        if let Some((_, name)) = PRIVILEGED_SETTINGS
            .iter()
            .enumerate()
            .find(|(index, _)| now.get(*index) != self.settings.get(*index))
        {
            return Some(format!(
                "a project file set `magi.{name}`, which decides what this session may do \
                 without asking; only your own configuration can set it"
            ));
        }
        // A role's program is spawned on every turn with the session's authority, so naming one is
        // more than declaring a tool — which a project file is already refused.
        let now = roles::said(engine);
        let (role, program) = now.iter().find(|held| !self.roles.contains(held))?;
        let setting = roles::of(role).map_or(role.as_str(), |known| known.named[0]);
        Some(format!(
            "a project file named `{program}` to fill the {role} role with `magi.{setting}`; \
             that program would run with this session's authority, so only your own \
             configuration can name it"
        ))
    }

    /// One message per declaration a project file made that will not be honoured, rather than a silent drop.
    fn refusals(&self, engine: &mut Engine) -> Vec<String> {
        // melchior owns the model; a project file naming a provider is told so by `Config::unkept`.
        let mut out = Vec::new();
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

/// The source behind one `magi.load` path, read from the config directory. A path that is not there
/// is skipped rather than fatal.
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

/// Which bucket a loaded path belongs to, if any. By the first path component, so `apis.lua` and
/// `apis/google.lua` land in the same place.
fn kind(path: &str) -> Option<&'static str> {
    ["apis", "tools", "clients"]
        .into_iter()
        .find(|name| path == format!("{name}.lua") || path.starts_with(&format!("{name}/")))
}

#[cfg(test)]
mod pinning_tests {
    use super::*;
    use magi_lua::Engine;

    fn from(source: &str) -> Loaded {
        let mut engine = Engine::new();
        engine.run(source, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
        }
    }

    #[test]
    fn a_configuration_may_pin_the_program_that_supplies_every_tool() {
        // casper is found on `$PATH` and owns `shell`, `read` and everything else the model calls.
        let loaded = from(r#"magi.casper_sha256 = "abc123""#);
        assert_eq!(tooling(&loaded).pin.as_deref(), Some("abc123"));
        assert_eq!(
            catalog(&loaded, Vec::new()).tooling.pin.as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn saying_nothing_pins_nothing() {
        // A pin is opt-in: `magi doctor` prints what casper actually hashed to.
        assert_eq!(tooling(&from("")).pin, None);
        assert_eq!(tooling(&from(r#"magi.casper_sha256 = "  ""#)).pin, None);
    }

    #[test]
    fn the_pin_and_the_settings_follow_whichever_program_fills_the_role() {
        // Keyed by the program's own name. A pin on casper is not a pin on the program that
        // replaced it — it would either bind the wrong bytes or, worse, look satisfied.
        let loaded = from(
            r#"magi.tools = "workbench"
               magi.casper_sha256 = "abc123"
               magi.casper = { off = true }
               magi.workbench_sha256 = "def456"
               magi.workbench = { quiet = true }"#,
        );
        let tooling = tooling(&loaded);
        assert_eq!(tooling.program, "workbench");
        assert_eq!(tooling.pin.as_deref(), Some("def456"));
        assert_eq!(tooling.configure, r#"{"quiet":true}"#);
    }

    #[test]
    fn the_default_program_reads_the_settings_it_always_read() {
        // The same rule, for the configuration everybody already has: `magi.casper` is the tools
        // program's table because casper is the tools program, not because it is spelled casper.
        let tooling = tooling(&from(r#"magi.casper = { off = true }"#));
        assert_eq!(tooling.program, "casper");
        assert_eq!(tooling.configure, r#"{"off":true}"#);
    }
}

#[cfg(test)]
mod mind_tests {
    use super::*;
    use magi_lua::Engine;

    fn from(source: &str) -> Loaded {
        let mut engine = Engine::new();
        engine.run(source, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
        }
    }

    #[test]
    fn a_named_melchior_is_the_one_the_catalog_is_read_from() {
        // `magi.melchior` was honoured where the layer is started and ignored where the models are
        // listed, so a session ran against one melchior while showing another's catalog.
        let loaded = from(r#"magi.melchior = "/opt/melchior-next""#);
        assert_eq!(mind(&loaded), "/opt/melchior-next");
        assert_eq!(
            catalog(&loaded, Vec::new()).mind,
            "/opt/melchior-next",
            "the catalog asks the one the config named"
        );
    }

    #[test]
    fn saying_nothing_means_the_sibling_by_its_own_name() {
        let loaded = from("");
        assert_eq!(mind(&loaded), magi_host::broker::MELCHIOR);
    }
}

#[cfg(test)]
mod staleness_tests {
    use super::*;
    use magi_model::scratch::Scratch;
    use std::time::{Duration, SystemTime};

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-stale", name)
    }

    #[test]
    fn a_file_edited_after_the_session_started_is_reported() {
        // `magi tools` lists the tool you just added; the running daemon was never told.
        let dir = scratch("edited");
        let file = dir.join("greet.lua");
        std::fs::write(&file, "x").expect("write");
        let started = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(newer_than(std::slice::from_ref(&file), started), vec![file]);
    }

    #[test]
    fn a_file_older_than_the_session_is_not() {
        let dir = scratch("older");
        let file = dir.join("greet.lua");
        std::fs::write(&file, "x").expect("write");
        let started = SystemTime::now() + Duration::from_secs(3600);
        assert!(newer_than(&[file], started).is_empty());
    }

    #[test]
    fn a_file_that_does_not_exist_is_not_a_change() {
        // `watched_files` names what a config *could* have; most installs have some of it.
        let dir = scratch("absent");
        let started = SystemTime::now() - Duration::from_secs(3600);
        assert!(newer_than(&[dir.join("nothing.lua")], started).is_empty());
    }

    #[test]
    fn no_pid_file_is_no_claim_either_way() {
        // Nothing is running, so nothing is out of date; warning here would fire on every first start.
        let dir = scratch("nopid");
        assert!(edited_since_start(&dir.join("a.sock")).is_empty());
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
    }
}
