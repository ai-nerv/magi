//! What a config cannot reach.
//!
//! `Lua::full()` hands the VM the whole standard library, which includes `os.execute` and
//! `io.popen`. A Lua tool with those can spawn processes, and then the process transport is a
//! stylistic preference rather than the only way to run a command — which is the opposite of
//! the design. So they are removed.
//!
//! Removed rather than never installed, because the alternative is assembling a standard
//! library by hand and quietly missing something the next luna release adds. A short list of
//! what must not be reachable is auditable; a long list of what may be is not.

use luna::{Lua, Value};

/// Globals a config must not have, and why each one is on the list.
///
/// `os.execute` and `io.popen` spawn. `os.remove`, `os.rename` and `os.tmpname` write outside
/// the `Ops` seam, which is where path checking lives. `os.exit` would let a config file end
/// the daemon. `io` goes wholesale: every remaining member of it opens a file, and a tool that
/// needs one has `Ops`.
const REMOVED: &[(&str, &str)] = &[
    ("os", "execute"),
    ("os", "exit"),
    ("os", "remove"),
    ("os", "rename"),
    ("os", "tmpname"),
    ("os", "setlocale"),
];

/// Globals removed entirely.
const REMOVED_TABLES: &[&str] = &["io", "package", "dofile", "loadfile", "require"];

/// Take away what a config must not be able to do.
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
    });
}

#[cfg(test)]
mod tests {
    use crate::Engine;

    /// What one expression evaluates to inside a fresh engine.
    fn probe(expression: &str) -> String {
        let mut engine = Engine::new();
        engine
            .run(
                &format!("axon.answer = tostring({expression})"),
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
        // The line that makes the process transport meaningful: if a description could spawn,
        // nobody would use the boundary, and `bash` being a peer would be decoration.
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
        assert_ne!(probe("axon.json.encode"), "nil");
        assert_ne!(probe("axon.stream.connect"), "nil");
    }
}
