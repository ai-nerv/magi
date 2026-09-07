//! Configuration nobody named.
//!
//! `init.lua` is still the entry point and `magi.load` is still how a file asks for another
//! one — a named file is the auditable case and stays the one to reach for. What this adds is
//! the directories: drop a `.lua` in `~/.config/magi/plugin/`, or install a package under
//! `~/.local/share/magi/site/pack/*/start/*/`, and it runs.
//!
//! **The mechanism is balthasar's, which had it first.** magi's loader carried a comment saying
//! nothing was discovered by scanning and called that "the property a plugin mechanism will
//! need" — future tense, beside a tested implementation in the next repository. See
//! [`magi_lua::plugins`].
//!
//! **A file that fails costs itself and nothing else.** `init.lua` is fatal because a config that
//! will not parse has not expressed an intention; a discovered file is somebody else's package,
//! and taking the session down over it would make installing one a risk rather than a try.

use magi_lua::Engine;
use magi_lua::acknowledged;
use magi_lua::plugins::{Roots, Trust, runtimepath};

/// Run every discovered file, in runtimepath order.
///
/// Only the owner's own roots. A project's `.magi.lua` is read further down in `load`, under the
/// trust rules that already exist there, and reading it here as well would run it twice.
///
/// The roots are a parameter rather than looked up here, so what this does is a function of what
/// it is handed. Reading the environment inside would make every test of the order depend on the
/// machine it ran on — and setting an environment variable to arrange one is `unsafe`, which is
/// denied across this workspace.
///
/// Returns what it ran, as `(name, source)`, in the order it ran them.
///
/// **The session rebuilds its VM from these.** The worker cannot be handed the VM this ran in — a
/// Lua state does not cross a thread — so it re-runs the declarations on its own thread, from the
/// sources the loader collected. A discovered file that was not collected therefore ran here,
/// declared into a VM that is thrown away, and reached no session at all: `magi tools` listed it
/// and a turn could not call it.
///
/// # Errors
/// Never for a plugin's own failure — those are reported and skipped. Only if draining what one
/// of them asked for fails, which is the same fatality `init.lua` already has.
pub fn run(
    engine: &mut Engine,
    roots: &Roots,
    drain: &mut dyn FnMut(&mut Engine) -> Result<(), magi_lua::LuaError>,
) -> Result<Vec<(String, String)>, magi_lua::LuaError> {
    let mut ran = Vec::new();
    let known = roots
        .config
        .as_ref()
        .map(|config| acknowledged::recorded(&acknowledged::manifest_in(config)))
        .unwrap_or_default();

    for (path, trust) in runtimepath(roots) {
        if trust == Trust::Project {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        // **A package runs when you have said it may, and not before.** Your own `plugin/` files
        // are yours and run on sight; this is for what arrived under `site/pack/` by being
        // fetched, and can change under you between one run and the next.
        if trust.needs_acknowledging() && !acknowledged::cleared(&known, &path, &source) {
            eprintln!(
                "magi: {}; run `magi acknowledge` to clear it",
                acknowledged::Held {
                    path: path.clone(),
                    known: acknowledged::seen(&known, &path),
                }
            );
            continue;
        }
        let named = path.display().to_string();
        if let Err(why) = engine.run(&source, &named) {
            eprintln!("magi: {named}: {why}");
            continue;
        }
        // A plugin may `magi.load` files of its own, so what it asked for is drained before the
        // next one runs — otherwise the second plugin's declarations would land before the
        // first's, and `after/` would stop meaning last.
        drain(engine)?;
        ran.push((named, source));
    }
    Ok(ran)
}

/// Every installed file, with what it holds right now.
///
/// What `magi trust` acknowledges. Only the installed ones: acknowledging your own configuration
/// would put a digest in the manifest for a file that never needed one, and the next edit to it
/// would then look like a package that changed.
#[must_use]
pub fn installed(roots: &Roots) -> Vec<(std::path::PathBuf, String)> {
    runtimepath(roots)
        .into_iter()
        .filter(|(_, trust)| trust.needs_acknowledging())
        .filter_map(|(path, _)| {
            std::fs::read_to_string(&path)
                .ok()
                .map(|source| (path, source))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn a_dropped_file_runs_and_a_broken_one_does_not_stop_the_rest() {
        // No edit to `init.lua`, no `magi.load` naming any of them. And a package that raises is
        // that package's problem: taking the session down over somebody else's file would make
        // installing one a risk rather than a try.
        let dir = Scratch::new("magi-disc", "dropped");
        let plugin = dir.join("plugin");
        std::fs::create_dir_all(&plugin).expect("mkdir");
        std::fs::write(plugin.join("a-first.lua"), "magi.first = 1\n").expect("write");
        std::fs::write(plugin.join("b-broken.lua"), "error(\"no\")\n").expect("write");
        std::fs::write(plugin.join("c-third.lua"), "magi.third = 3\n").expect("write");

        let mut engine = Engine::new();
        let roots = Roots {
            config: Some(dir.to_path_buf()),
            site: None,
            project: None,
        };
        run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        engine.harvest();
        let config = engine.config();

        assert_eq!(config.number("first"), Some(1.0), "the first ran");
        assert_eq!(
            config.number("third"),
            Some(3.0),
            "and so did the one after the broken one"
        );
    }

    #[test]
    fn after_wins_over_a_plugin_that_set_the_same_thing() {
        // The registrars replace by name and the last write decides, so `after/` is how a person
        // overrides something a package they installed declared.
        let dir = Scratch::new("magi-disc", "after");
        std::fs::create_dir_all(dir.join("plugin")).expect("mkdir");
        std::fs::create_dir_all(dir.join("after/plugin")).expect("mkdir");
        std::fs::write(dir.join("plugin/it.lua"), "magi.model = \"theirs\"\n").expect("write");
        std::fs::write(dir.join("after/plugin/it.lua"), "magi.model = \"mine\"\n").expect("write");

        let mut engine = Engine::new();
        let roots = Roots {
            config: Some(dir.to_path_buf()),
            site: None,
            project: None,
        };
        run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        engine.harvest();
        assert_eq!(engine.config().string("model"), Some("mine"));
    }

    #[test]
    fn a_project_file_is_not_run_here() {
        // It is read further down in `load`, under the trust rules that live there. Running it
        // here as well would run it twice — and the second time as the owner's own.
        let dir = Scratch::new("magi-disc", "project");
        std::fs::write(dir.join(".magi.lua"), "magi.model = \"theirs\"\n").expect("write");

        let mut engine = Engine::new();
        let roots = Roots {
            config: None,
            site: None,
            project: Some(dir.to_path_buf()),
        };
        run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        engine.harvest();
        assert_eq!(engine.config().string("model"), None);
    }

    #[test]
    fn a_discovered_file_cannot_spawn_a_process() {
        // The half that makes discovery safe to have at all. The sandbox is applied in
        // `Engine::new`, so this is a property of the VM rather than of the loader — and the
        // point of asserting it here is that discovery is what made it matter: until now every
        // file that ran had been named by somebody.
        let mut engine = Engine::new();
        assert!(
            engine.run("return os.execute(\"true\")", "plugin").is_err(),
            "os.execute is gone"
        );
        assert!(
            engine.run("return io.popen(\"true\")", "plugin").is_err(),
            "and so is io.popen"
        );
    }
}

#[cfg(test)]
mod acknowledging {
    use super::*;
    use magi_model::scratch::Scratch;

    /// A config directory and a site directory, with one package installed under it.
    fn installed_package(name: &str, body: &str) -> (Scratch, Roots) {
        let dir = Scratch::new("magi-ack-disc", name);
        let at = dir.join("site/pack/vendor/start/thing/plugin");
        std::fs::create_dir_all(&at).expect("mkdir");
        std::fs::write(at.join("it.lua"), body).expect("write");
        let roots = Roots {
            config: Some(dir.join("config")),
            site: Some(dir.join("site")),
            project: None,
        };
        (dir, roots)
    }

    #[test]
    fn a_package_nobody_acknowledged_does_not_run() {
        // Fail-closed, and the whole of what P8 is for: fetching is `git clone`, the idea is the
        // lockfile. An unacknowledged package is not a warning that scrolls past — it is code
        // that did not run.
        let (_dir, roots) = installed_package("fresh", "magi.model = \"theirs\"\n");
        let mut engine = Engine::new();
        run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        engine.harvest();
        assert_eq!(engine.config().string("model"), None);
    }

    #[test]
    fn a_package_that_was_acknowledged_runs() {
        let (_dir, roots) = installed_package("agreed", "magi.model = \"theirs\"\n");
        let config = roots.config.clone().expect("a config root");
        let manifest = acknowledged::manifest_in(&config);
        acknowledged::acknowledge(&manifest, &installed(&roots)).expect("acknowledge");

        let mut engine = Engine::new();
        run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        engine.harvest();
        assert_eq!(engine.config().string("model"), Some("theirs"));
    }

    #[test]
    fn a_package_that_changed_after_being_acknowledged_stops_running() {
        // The case the digest is for. An acknowledgement that survived an update would be an
        // acknowledgement of code nobody has read.
        let (dir, roots) = installed_package("changed", "magi.model = \"theirs\"\n");
        let config = roots.config.clone().expect("a config root");
        let manifest = acknowledged::manifest_in(&config);
        acknowledged::acknowledge(&manifest, &installed(&roots)).expect("acknowledge");

        std::fs::write(
            dir.join("site/pack/vendor/start/thing/plugin/it.lua"),
            "magi.model = \"something else entirely\"\n",
        )
        .expect("rewrite");

        let mut engine = Engine::new();
        run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        engine.harvest();
        assert_eq!(engine.config().string("model"), None);
    }

    #[test]
    fn your_own_plugin_directory_never_needs_acknowledging() {
        // A prompt about your own configuration is one nobody reads, and training people to say
        // yes is worse than not asking. This is the line: acknowledge what somebody else wrote.
        let dir = Scratch::new("magi-ack-disc", "own");
        std::fs::create_dir_all(dir.join("plugin")).expect("mkdir");
        std::fs::write(dir.join("plugin/mine.lua"), "magi.model = \"mine\"\n").expect("write");

        let roots = Roots {
            config: Some(dir.to_path_buf()),
            site: None,
            project: None,
        };
        assert!(installed(&roots).is_empty(), "nothing to acknowledge");

        let mut engine = Engine::new();
        run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        engine.harvest();
        assert_eq!(engine.config().string("model"), Some("mine"));
    }
}

#[cfg(test)]
mod collecting {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn what_ran_is_handed_back_so_the_session_can_run_it_too() {
        // **Found by running it.** A Lua state does not cross a thread, so the worker rebuilds
        // its VM on its own thread from the sources the loader collected. A discovered file that
        // was not collected ran here, declared into a VM that is thrown away, and reached no
        // session: it appeared in `magi tools` and a turn could not call it.
        let dir = Scratch::new("magi-disc", "collected");
        std::fs::create_dir_all(dir.join("plugin")).expect("mkdir");
        std::fs::write(
            dir.join("plugin/mine.lua"),
            "magi.tool(\"mine\", { description = \"x\", parameters = {}, run = function() end })\n",
        )
        .expect("write");

        let roots = Roots {
            config: Some(dir.to_path_buf()),
            site: None,
            project: None,
        };
        let mut engine = Engine::new();
        let ran = run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");

        assert_eq!(ran.len(), 1, "{ran:?}");
        assert!(ran[0].0.ends_with("plugin/mine.lua"), "named by its path");
        assert!(ran[0].1.contains("magi.tool"), "and carries its source");
    }

    #[test]
    fn a_file_that_did_not_run_is_not_handed_back() {
        // A broken plugin and an unacknowledged package are both skipped, and neither must end
        // up in what the session re-runs — the second time would raise on the worker's thread,
        // where the whole VM is abandoned over one bad description.
        let dir = Scratch::new("magi-disc", "not-collected");
        std::fs::create_dir_all(dir.join("plugin")).expect("mkdir");
        std::fs::write(dir.join("plugin/broken.lua"), "error(\"no\")\n").expect("write");

        let roots = Roots {
            config: Some(dir.to_path_buf()),
            site: None,
            project: None,
        };
        let mut engine = Engine::new();
        let ran = run(&mut engine, &roots, &mut |_| Ok(())).expect("discovery");
        assert!(ran.is_empty(), "{ran:?}");
    }
}

#[cfg(test)]
mod forgetting {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn acknowledging_after_a_package_is_gone_forgets_it() {
        // **The manifest replaces rather than merges**, so this is what clears a removed package.
        // Short-circuiting on an empty list looked tidy and left the digest of a file nobody has
        // any more sitting in the manifest — where, if the package ever came back with different
        // contents at the same path, it would read as still acknowledged.
        let dir = Scratch::new("magi-ack-disc", "forgets");
        let at = dir.join("site/pack/vendor/start/thing/plugin");
        std::fs::create_dir_all(&at).expect("mkdir");
        std::fs::write(at.join("it.lua"), "magi.model = \"theirs\"\n").expect("write");

        let roots = Roots {
            config: Some(dir.join("config")),
            site: Some(dir.join("site")),
            project: None,
        };
        let manifest = acknowledged::manifest_in(&roots.config.clone().expect("config"));
        acknowledged::acknowledge(&manifest, &installed(&roots)).expect("acknowledge");
        assert_eq!(acknowledged::recorded(&manifest).len(), 1);

        std::fs::remove_dir_all(dir.join("site")).expect("uninstall");
        acknowledged::acknowledge(&manifest, &installed(&roots)).expect("acknowledge nothing");
        assert!(
            acknowledged::recorded(&manifest).is_empty(),
            "the removed package is forgotten"
        );
    }
}
