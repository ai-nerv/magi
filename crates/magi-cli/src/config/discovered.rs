//! Configuration nobody named: a `.lua` dropped in `~/.config/magi/plugin/`, or a package installed
//! under `~/.local/share/magi/site/pack/*/start/*/`, runs without `init.lua` naming it. A discovered
//! file that fails is reported and skipped; only `init.lua` is fatal. See [`magi_lua::plugins`].

use magi_lua::Engine;
use magi_lua::acknowledged;
use magi_lua::plugins::{Roots, Trust, runtimepath};

/// Run every discovered file, in runtimepath order, returning what it ran as `(name, source)`.
/// Only the owner's own roots: a project's `.magi.lua` is read further down in `load`, under the
/// trust rules there, and reading it here too would run it twice. The roots are a parameter, so the
/// result does not depend on the machine it ran on. The session rebuilds its VM from the returned
/// sources — a Lua state does not cross a thread — so a file that ran here but was not collected
/// reached no session at all.
///
/// # Errors
/// Never for a plugin's own failure — those are reported and skipped. Only if draining what one of
/// them asked for fails.
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
        // Your own `plugin/` files run on sight; this is for what arrived under `site/pack/`.
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
        // next one runs, or `after/` would stop meaning last.
        drain(engine)?;
        ran.push((named, source));
    }
    Ok(ran)
}

/// Every installed file, with what it holds right now — what `magi trust` acknowledges. Only the
/// installed ones: the owner's own files never needed a digest, and an edit would look like change.
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
        // No edit to `init.lua`, no `magi.load` naming any of them; a package that raises is skipped.
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
        // The registrars replace by name and the last write decides, so `after/` overrides.
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
        // It is read further down in `load`, under the trust rules there; here it would run twice.
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
        // The sandbox is applied in `Engine::new`, so it is a property of the VM, not of the loader.
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
        // Fail-closed: an unacknowledged package is not a warning, it is code that did not run.
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
        // An acknowledgement that survived an update would acknowledge code nobody has read.
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
        // Only what somebody else wrote is acknowledged; a prompt about your own config is noise.
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
        // A Lua state does not cross a thread, so the worker rebuilds its VM from the collected
        // sources: a discovered file that was not collected reaches no session.
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
        // A broken plugin and an unacknowledged package are skipped and must not be re-run on the
        // worker's thread, where the whole VM is abandoned over one raise.
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
        // The manifest replaces rather than merges, so this is what clears a removed package: a
        // left-behind digest would read as still acknowledged if the path ever came back.
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
