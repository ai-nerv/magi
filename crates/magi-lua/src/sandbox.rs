//! What a config cannot reach.
//!
//! `Lua::full()` includes `os.execute` and `io.popen`; a Lua tool with those can spawn, and the
//! process transport becomes a preference rather than the only way to run a command. Removed
//! rather than never installed: a short list of what must not be reachable is auditable.

use luna::{Lua, Value};

/// Globals a config must not have, and why each is on the list. `os.execute` and `io.popen` spawn.
/// `os.remove`, `os.rename` and `os.tmpname` write outside the `Ops` seam. `os.exit` would let a
/// config end the daemon. `io` goes wholesale; a tool that needs a file has `Ops`.
const REMOVED: &[(&str, &str)] = &[
    ("os", "execute"),
    ("os", "exit"),
    ("os", "remove"),
    ("os", "rename"),
    ("os", "tmpname"),
    ("os", "setlocale"),
];

const REMOVED_TABLES: &[&str] = &["io", "package", "dofile", "loadfile", "require"];

/// Globals a project's file must not have, beyond what [`luna::Lua::core`] never loads: `load`
/// compiles code, `collectgarbage` steers the collector, `warn` and `print` write to the terminal,
/// and the rest read files, spawn, read the environment or reach inside other frames.
const REMOVED_FROM_PROJECTS: &[&str] = &[
    "load",
    "loadstring",
    "dofile",
    "loadfile",
    "require",
    "collectgarbage",
    "warn",
    "print",
    "io",
    "os",
    "package",
    "debug",
];

/// Take away everything but data from a VM that runs a project's own file.
///
/// # Panics
/// If a removal did not take, for the reason [`apply`] gives.
pub fn apply_project(lua: &mut Lua) {
    lua.enter(|ctx| {
        for name in REMOVED_FROM_PROJECTS {
            ctx.set_global(name, Value::Nil);
        }
        for name in REMOVED_FROM_PROJECTS {
            assert!(
                ctx.get_global_value(name).is_nil(),
                "the project sandbox could not remove the global `{name}`"
            );
        }
        if let Value::Table(magi) = ctx.get_global_value("magi") {
            for name in ["stream", "fs", "json", "self", "clients"] {
                assert!(
                    magi.get_value(ctx, name).is_nil(),
                    "a project VM was handed `magi.{name}`"
                );
            }
        }
    });
}

/// Take away what a config must not be able to do.
///
/// # Panics
/// If a removal did not take. `Table::set` returns a `Result` and luna has `set_readonly`, so a
/// read-only standard-library table would decline every removal in silence and leave the VM with
/// `os.execute`. A magi that cannot sandbox its VM refuses to start.
pub fn apply(lua: &mut Lua) {
    lua.enter(|ctx| {
        for (table, field) in REMOVED {
            if let Value::Table(t) = ctx.get_global_value(table) {
                t.set(ctx, *field, Value::Nil).ok();
            }
        }
        for name in REMOVED_TABLES {
            ctx.set_global(name, Value::Nil);
        }
        // Read back, because every write above can decline in silence.
        for (table, field) in REMOVED {
            if let Value::Table(t) = ctx.get_global_value(table) {
                assert!(
                    t.get_value(ctx, *field).is_nil(),
                    "the sandbox could not remove {table}.{field}"
                );
            }
        }
        for name in REMOVED_TABLES {
            assert!(
                ctx.get_global_value(name).is_nil(),
                "the sandbox could not remove the global `{name}`"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::Engine;

    fn probe(expression: &str) -> String {
        let mut engine = Engine::new();
        engine
            .run(
                &format!("magi.answer = tostring({expression})"),
                "probe.lua",
            )
            .expect("run");
        engine.harvest();
        engine
            .config()
            .string("answer")
            .unwrap_or("<absent>")
            .to_owned()
    }

    #[test]
    fn a_config_cannot_spawn_a_process() {
        // If a description could spawn, nobody would use the boundary.
        assert_eq!(probe("os.execute"), "nil");
        assert_eq!(probe("io"), "nil");
    }

    #[test]
    fn a_config_cannot_write_outside_the_ops_seam() {
        for expression in ["os.remove", "os.rename", "os.tmpname"] {
            assert_eq!(probe(expression), "nil", "{expression} is still reachable");
        }
    }

    #[test]
    fn a_config_cannot_end_the_daemon() {
        assert_eq!(probe("os.exit"), "nil");
    }

    #[test]
    fn a_config_cannot_load_arbitrary_files() {
        for expression in ["dofile", "loadfile", "require", "package"] {
            assert_eq!(probe(expression), "nil", "{expression} is still reachable");
        }
    }

    #[test]
    fn a_config_that_loops_is_stopped_and_named() {
        let mut engine = Engine::new();
        let error = engine
            .run("while true do end", "loop.lua")
            .expect_err("a loop is stopped");
        let said = error.to_string();
        assert!(
            said.contains("loop.lua") && said.contains("budget"),
            "{said}"
        );
        // And the VM is still usable afterwards.
        engine.run("magi.after = 1", "after.lua").expect("runs");
    }

    /// What one expression comes to in a project's VM.
    fn project_probe(expression: &str) -> String {
        let mut engine = Engine::project(&serde_json::Map::new());
        engine
            .run(
                &format!("magi.answer = tostring({expression})"),
                "probe.lua",
            )
            .expect("run");
        engine
            .assigned()
            .into_iter()
            .find(|(name, _)| name == "answer")
            .and_then(|(_, value)| value?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "<absent>".to_owned())
    }

    #[test]
    fn a_project_file_has_no_socket_filesystem_environment_or_load() {
        for expression in [
            "load",
            "loadstring",
            "dofile",
            "loadfile",
            "require",
            "os",
            "io",
            "package",
            "debug",
            "collectgarbage",
            "print",
            "magi.stream",
            "magi.fs",
            "magi.json",
            "__stream",
        ] {
            assert_eq!(
                project_probe(expression),
                "nil",
                "{expression} is reachable"
            );
        }
        assert_ne!(
            project_probe("string.format"),
            "nil",
            "plain data still works"
        );
    }

    #[test]
    fn a_project_file_that_loops_is_stopped() {
        let mut engine = Engine::project(&serde_json::Map::new());
        let error = engine
            .run("while true do end", ".magi.lua")
            .expect_err("stopped");
        assert!(error.to_string().contains("budget"), "{error}");
        let mut engine = Engine::project(&serde_json::Map::new());
        let error = engine
            .run(
                "local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1024) end",
                ".magi.lua",
            )
            .expect_err("stopped");
        assert!(
            error.to_string().contains("memory") || error.to_string().contains("budget"),
            "{error}"
        );
    }

    #[test]
    fn a_project_file_may_not_declare_a_tool_a_watcher_or_a_load() {
        for call in [
            "magi.tool('mine', {})",
            "magi.watch('mine', {})",
            "magi.load('tools.lua')",
        ] {
            let mut engine = Engine::project(&serde_json::Map::new());
            let error = engine.run(call, ".magi.lua").expect_err("refused");
            assert!(
                error.to_string().contains("project file"),
                "{call}: {error}"
            );
        }
    }

    #[test]
    fn what_a_config_legitimately_needs_still_works() {
        // The removals must not cost a config the things it is for.
        assert_ne!(probe("os.getenv"), "nil", "reading the environment is fine");
        assert_ne!(probe("os.time"), "nil");
        assert_ne!(
            probe("load"),
            "nil",
            "the family's clients are loaded chunks"
        );
        assert_ne!(probe("string.format"), "nil");
        assert_ne!(probe("table.concat"), "nil");
        assert_ne!(probe("magi.json.encode"), "nil");
        assert_ne!(probe("magi.stream.connect"), "nil");
    }
}
