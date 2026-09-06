//! Loading the config, and the catalog that is part of it.
//!
//! The config is Lua because the interesting configs are programs: probe the machine, loop over
//! a directory of endpoints, branch on whether a GPU box answers. A provider declared in a loop
//! is the same table as one written out by hand, and neither is a fragment anybody has to merge.
//!
//! **The built-in catalog is not special.** It is the first config file, run through the same VM
//! and the same registrar as the user's, so a user file that declares `magi.provider("groq",
//! ...)` replaces it by the ordinary rule that registration is keyed. One mechanism, not two.

use magi_lua::{Config, Engine, LuaError};
use std::collections::BTreeSet;

/// Everything the config files said, in one value.
pub struct Loaded {
    /// Settings and registrations, as the config left them.
    pub config: Config,
    /// Every tool description that was run, as `(name, source)`.
    pub tools: Vec<(String, String)>,
    /// The family's client libraries, as `(name, source)`.
    pub clients: Vec<(String, String)>,
}

/// Run `init.lua`, then everything it asked for, and collect what they declared.
///
/// **One entry point.** The host runs `init.lua` and nothing else by name; every other file is
/// reached through `magi.load`. Nothing is discovered by scanning, so a file that is not named
/// does not run — the property a plugin mechanism will need, and one a scanner cannot offer.
///
/// **Nothing is compiled in.** A protocol description, a catalog and a tool are configuration:
/// they change without the binary changing, and a binary carrying a copy is a binary you rebuild
/// to fix a wire format. So every one of them is read from the config directory at run time, and
/// `make configs` is what puts them there.
pub fn load() -> Result<Loaded, LuaError> {
    let mut engine = Engine::new();
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
                // Read and run like any other file, but nothing is kept: a protocol description
                // is melchior's now, and a copy held here would be a copy that drifts. A config
                // that still names one is not an error — it simply declares to nobody.
                Some("apis") => engine.run(source, path)?,
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
    // Unless the user said otherwise: `magi.trusted` names directories whose project files are
    // as good as their own. The decision belongs to the person, it is made once, and it lives
    // in the config only they can edit -- which is the whole of what a trust boundary needs to
    // be. Without a way to say yes, the rule would be worked around instead of used.
    engine.harvest();
    let machine = trusts_here(&engine.config()).then(|| Trusted::snapshot(&mut engine));

    // Then the project, last, so a repository can choose among what the machine offers.
    for path in magi_lua::search_paths() {
        if path.exists() && path.file_name().is_some_and(|n| n == ".magi.lua") {
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

    // Everything a config said that magi did not keep, whoever said it. A declaration for
    // something a sibling owns and a setting nested past what can be described both end up here,
    // and both were silent before: the config author wrote a line that did nothing and there was
    // no way to find that out except by noticing the absence of an effect.
    for said in &engine.config().unkept {
        eprintln!("magi: {said}");
    }

    if let Some(machine) = &machine {
        // A changed privileged setting is fatal, not a warning. The others describe something a
        // project file offered that will not be used, and carrying on without it is right. This
        // one is a project file having *already* changed how the rest of the session is
        // governed — `confine` off, a grant added, itself vouched for — and there is nothing
        // sensible to carry on with: the value it wanted is the value the config now holds.
        if let Some(name) = machine.altered(&mut engine) {
            return Err(LuaError::Runtime {
                file: ".magi.lua".to_owned(),
                message: format!(
                    "a project file set `magi.{name}`, which decides what this session may do \
                     without asking; only your own configuration can set it"
                ),
            });
        }
        for refused in machine.refusals(&mut engine) {
            eprintln!("magi: {refused}");
        }
    }
    collect(engine.config(), tools, clients)
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

/// Everything the registrar collected, as one value.
///
/// No providers. A catalog of models was read here once, from `providers.lua`, and it is
/// melchior's now: which models exist, which protocol each speaks and what credential each takes
/// are one subject, and magi keeping half of it was a second thing to keep in step.
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

/// Everything the daemon could talk to, so `:model` has something to pick among.
///
/// Built once at start rather than re-read on each switch: a session should keep answering
/// with what it was started with, and picking up an edit made since would leave a person
/// asking why it is using a model they did not choose.
/// The cards come from melchior, which owns them.
#[must_use]
pub fn catalog(loaded: &Loaded, cards: Vec<magi_proto::ask::Card>) -> magi_host::catalog::Catalog {
    let mut catalog = magi_host::catalog::Catalog {
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
use chosen::asked;
mod settings;

use settings::{grants, options, system};

pub use settings::{adopt_ui, environ, grants as granted};

/// What this directory chose last time it was used.
#[must_use]
pub fn remembered() -> magi_host::remember::Chosen {
    std::env::current_dir()
        .map(|cwd| magi_host::remember::of(&cwd.display().to_string()))
        .unwrap_or_default()
}

/// The backend a daemon should run turns against, if one is both chosen and usable.
///
/// A model that is configured but has no credential yields `None` rather than an error: the
/// daemon still starts, and the refusal it journals names what to set. A daemon that would not
/// start because a key was missing is a worse answer than a session that says so.
#[must_use]
pub fn backend(catalog: &magi_host::catalog::Catalog) -> Option<magi_host::turn::Backend> {
    // A lookup rather than a second assembly. It was a second assembly, and the two disagreed
    // about what "configured" meant more than once.
    catalog
        .chosen()
        .as_deref()
        .and_then(|name| catalog.backend(name))
}

/// Configuration files edited since the daemon on `socket` started.
///
/// A session holds the tool set it was built with. Nothing said so, and the two disagreed in
/// the worst direction: `magi tools` reads the configuration and lists a tool you just added,
/// the running session was never told about it, and the model reports that the tool is not
/// registered -- which reads as a broken tool rather than a stale session. Same shape as "I ran
/// `make configs` and still nothing".
///
/// The socket is bound when the session starts, so its mtime is when the session began. No
/// protocol change and nothing to keep in sync: a file newer than that was not read.
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
        .map(|base| base.join("magi"))
}

/// What the machine's own configuration had declared, before any project file ran.
///
/// A `.magi.lua` arrives with a checkout: cloning a repository and running `magi` in it must not
/// be enough to add a tool, because a process tool names a command to run.
///
/// It guarded providers too, and no longer needs to: `magi.provider` was a registrar that kept
/// what it was handed where nothing read it, so a provider could not be added by anybody, and
/// the guard was watching a door that opened onto nothing. melchior owns the model.
///
/// A project file can still *choose*: `magi.model` picks among the models melchior already
/// offers. That is the useful half, and it carries no authority.
pub struct Trusted {
    tools: BTreeSet<String>,
    /// What [`PRIVILEGED_SETTINGS`] were before a project file ran.
    ///
    /// magi had no equivalent of this at all, and it is the half that matters most: a checked-in
    /// `.magi.lua` could set `magi.confine = false`, add to `magi.grants`, or name its own
    /// directory in `magi.trusted` — turning off the wall, granting itself permissions, or
    /// vouching for itself — and none of it was refused or even reported. Declarations were
    /// guarded and the switches that govern them were not.
    settings: Vec<Option<serde_json::Value>>,
}

/// Settings a project's own file may not assign.
///
/// `confine` is the wall; `grants` is what may happen without asking; `trusted` decides which
/// files this rule applies to at all — a file that could set the last one could exempt itself.
/// Named here rather than inferred, the way balthasar names its own: the list is short, and a
/// rule about which settings are dangerous should be readable in one place.
const PRIVILEGED_SETTINGS: &[&str] = &["confine", "grants", "trusted"];

impl Trusted {
    /// Record what has been declared so far.
    fn snapshot(engine: &mut Engine) -> Self {
        Self {
            tools: engine.tools().into_iter().map(|(name, _)| name).collect(),
            settings: Self::privileged(engine),
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

    /// Which privileged setting a project file changed, if it changed one.
    ///
    /// Compared by value against the snapshot taken before it ran. Byte-equal is no change: a
    /// project file may read `magi.confine` and assign it back — configs do that, and refusing
    /// it would make the rule fire on a file that changed nothing.
    fn altered(&self, engine: &mut Engine) -> Option<&'static str> {
        let now = Self::privileged(engine);
        PRIVILEGED_SETTINGS
            .iter()
            .enumerate()
            .find(|(index, _)| now.get(*index) != self.settings.get(*index))
            .map(|(_, name)| *name)
    }

    /// One message per declaration a project file made that will not be honoured.
    ///
    /// Reported rather than silently dropped: a config author who wrote something that does
    /// nothing needs to know, and a repository trying it is worth seeing.
    fn refusals(&self, engine: &mut Engine) -> Vec<String> {
        // Providers are not here any more, and it is worth saying why: `magi.provider` kept
        // nothing for anybody, so the machine's own configuration could not declare one either
        // and this loop only ever fired on the case where both sides were equally ignored.
        // melchior owns the model; a project file naming a provider is now told so by
        // `Config::unkept`, in the same words a machine configuration gets.
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

/// The source behind one `magi.load` path, read from the config directory.
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
mod staleness_tests {
    use super::*;
    use magi_model::scratch::Scratch;
    use std::time::{Duration, SystemTime};

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-stale", name)
    }

    #[test]
    fn a_file_edited_after_the_session_started_is_reported() {
        // The whole point: `magi tools` lists the tool you just added, the running daemon was
        // never told, and the model reports it as unregistered.
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
        // Nothing is running, so nothing is out of date. Warning here would fire on every
        // first start in a directory.
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
