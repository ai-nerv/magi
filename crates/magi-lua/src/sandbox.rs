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
