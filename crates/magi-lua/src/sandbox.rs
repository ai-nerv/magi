//! What a config cannot reach.
//!
//! `Lua::full()` includes `os.execute` and `io.popen`; a Lua tool with those can spawn, and the
//! process transport becomes a preference rather than the only way to run a command. Removed
//! rather than never installed: a short list of what must not be reachable is auditable.

use luna::{Callback, CallbackReturn, Context, IntoValue, Lua, MetaMethod, Value};

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

/// Globals removed whole. `debug` reaches other frames and sets metatables past `setmetatable`.
const REMOVED_TABLES: &[&str] = &["io", "package", "dofile", "loadfile", "require", "debug"];

/// Globals replaced rather than removed: luna's `print` writes to stdout and its `warn` to stderr,
/// which the TUI owns. Both go to magi's log instead, and nowhere when none was asked for.
const QUIETENED: &[&str] = &["print", "warn"];

/// What a metatable carrying `__gc` is told.
const FINALIZED: &str = "setmetatable: a metatable with __gc is refused; no finalizer runs here";

/// What a precompiled chunk is told, wherever it is handed in.
pub const PRECOMPILED: &str = "a precompiled chunk is refused; only Lua source is loaded";

/// Whether `source` is a precompiled chunk: every binary signature, PUC-Rio's and luna's, opens
/// with ESC.
#[must_use]
pub fn precompiled(source: &[u8]) -> bool {
    source.first() == Some(&0x1b)
}

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
        guard(ctx, "setmetatable", finalized);
        hide_userdata_metatables(ctx);
    });
}

/// `getmetatable`, answering `nil` for a userdata: luna attaches one shared metatable to every
/// error it raises as a userdata, and `__gc` set on it would enrol each later error.
fn hide_userdata_metatables(ctx: Context<'_>) {
    let Value::Function(original) = ctx.get_global_value("getmetatable") else {
        return;
    };
    let hidden = Callback::from_fn_with(&ctx, original, |original, ctx, _exec, mut stack| {
        if matches!(stack.get(0), Value::UserData(_)) {
            stack.replace(ctx, Value::Nil);
            return Ok(CallbackReturn::Return);
        }
        Ok(CallbackReturn::Call {
            function: *original,
            then: None,
        })
    });
    ctx.set_global("getmetatable", hidden);
}

/// A check on a call's first two arguments, naming why it refuses them.
type Refusal = for<'gc> fn(Context<'gc>, Value<'gc>, Value<'gc>) -> Option<&'static str>;

/// Put `refuses` in front of the global `name`, which then runs as it did.
fn guard(ctx: Context<'_>, name: &'static str, refuses: Refusal) {
    let Value::Function(original) = ctx.get_global_value(name) else {
        return;
    };
    let guarded = Callback::from_fn_with(&ctx, original, move |original, ctx, _exec, stack| {
        if let Some(why) = refuses(ctx, stack.get(0), stack.get(1)) {
            return Err(why.into_value(ctx).into());
        }
        Ok(CallbackReturn::Call {
            function: *original,
            then: None,
        })
    });
    ctx.set_global(name, guarded);
}

/// `load` handed a precompiled chunk, whatever mode it names.
fn binary<'gc>(_ctx: Context<'gc>, chunk: Value<'gc>, _: Value<'gc>) -> Option<&'static str> {
    matches!(chunk, Value::String(chunk) if precompiled(chunk.as_bytes())).then_some(PRECOMPILED)
}

/// A metatable carrying `__gc`, which would enrol its object for a finalizer.
fn finalized<'gc>(ctx: Context<'gc>, _: Value<'gc>, metatable: Value<'gc>) -> Option<&'static str> {
    matches!(metatable, Value::Table(metatable) if !metatable.get_value(ctx, MetaMethod::Gc).is_nil())
        .then_some(FINALIZED)
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
        guard(ctx, "setmetatable", finalized);
        hide_userdata_metatables(ctx);
        guard(ctx, "load", binary);
        for name in QUIETENED {
            ctx.set_global(name, quiet(ctx, name));
        }
    });
}

/// A `print` or `warn` that writes one line to magi's log, `MAGI_DEBUG_LOG` or `NERV_LOG`.
fn quiet<'gc>(ctx: Context<'gc>, named: &'static str) -> Callback<'gc> {
    Callback::from_fn(&ctx, move |_ctx, _exec, mut stack| {
        if magi_model::noted::enabled() {
            let said: Vec<String> = (0..stack.len()).map(|at| text(&stack.get(at))).collect();
            magi_model::noted::note(format_args!("lua {named}: {}", said.join("\t")));
        }
        stack.clear();
        Ok(CallbackReturn::Return)
    })
}

/// One value as text, without `__tostring`, which would re-enter the VM from a diagnostic.
fn text(value: &Value<'_>) -> String {
    match value {
        Value::String(said) => said.display_lossy().to_string(),
        other => other.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use std::collections::BTreeSet;

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

    /// Every name a fresh VM holds, two tables deep: globals, their fields, and those fields'.
    const WALK: &str = "local names = {}
        local function walk(held, prefix, depth)
          for key, value in pairs(held) do
            local name = prefix .. tostring(key)
            names[#names + 1] = name
            if type(value) == 'table' and value ~= _G and depth < 2 then
              walk(value, name .. '.', depth + 1)
            end
          end
        end
        walk(_G, '', 0)
        table.sort(names)
        return table.concat(names, ' ')";

    /// The names `walked` found beyond `allowed`, and those of `allowed` it did not find, with
    /// `optional` neither required nor refused.
    fn against(walked: &str, allowed: &str, optional: &str) -> (Vec<String>, Vec<String>) {
        let found: BTreeSet<&str> = walked.split_whitespace().collect();
        let allowed: BTreeSet<&str> = allowed.split_whitespace().collect();
        let optional: BTreeSet<&str> = optional.split_whitespace().collect();
        let added = found
            .iter()
            .filter(|name| !allowed.contains(*name) && !optional.contains(*name))
            .map(ToString::to_string)
            .collect();
        let gone = allowed
            .difference(&found)
            .map(ToString::to_string)
            .collect();
        (added, gone)
    }

    /// What a config VM lends, and nothing else: an addition fails here until it is listed.
    const CONFIG_ALLOWED: &str = "
        _G _VERSION assert collectgarbage error getmetatable ipairs load next pairs pcall print
        rawequal rawget rawlen rawset select setmetatable tonumber tostring type warn xpcall
        __magi_tools __magi_watching __stream __stream.connect
        magi magi.agent magi.fs magi.fs.ls magi.fs.write magi.json magi.json.decode
        magi.json.encode magi.load magi.mux magi.provider magi.self magi.shell magi.stream
        magi.stream.connect magi.tool magi.ui magi.watch
        coroutine coroutine.close coroutine.continue coroutine.create coroutine.isyieldable
        coroutine.resume coroutine.running coroutine.status coroutine.wrap coroutine.yield
        coroutine.yieldto
        math math.abs math.acos math.asin math.atan math.ceil math.cos math.deg math.exp
        math.floor math.fmod math.huge math.log math.max math.maxinteger math.min math.mininteger
        math.modf math.pi math.rad math.random math.randomseed math.sin math.sqrt math.tan
        math.tointeger math.type math.ult
        os os.clock os.date os.difftime os.getenv os.time
        string string.byte string.char string.dump string.find string.format string.gmatch
        string.gsub string.len string.lower string.match string.pack string.packsize string.rep
        string.reverse string.sub string.unpack string.upper
        table table.clear table.concat table.freeze table.insert table.interceptall
        table.isfrozen table.move table.pack table.remove table.sort table.unpack
        utf8 utf8.char utf8.charpattern utf8.codepoint utf8.codes utf8.len utf8.offset
    ";

    /// What the process names once a session is open, which a test elsewhere in this binary may do.
    const CONFIG_OPTIONAL: &str = "magi.session magi.balthasar_at magi.melchior_at";

    /// What a project file's data-only VM lends: the core library, minus code and the outside.
    const PROJECT_ALLOWED: &str = "
        _G _VERSION assert error getmetatable ipairs next pairs pcall rawequal rawget rawlen
        rawset select setmetatable tonumber tostring type xpcall
        magi magi.agent magi.load magi.mux magi.provider magi.shell magi.tool magi.ui magi.watch
        coroutine coroutine.close coroutine.continue coroutine.create coroutine.isyieldable
        coroutine.resume coroutine.running coroutine.status coroutine.wrap coroutine.yield
        coroutine.yieldto
        math math.abs math.acos math.asin math.atan math.ceil math.cos math.deg math.exp
        math.floor math.fmod math.huge math.log math.max math.maxinteger math.min math.mininteger
        math.modf math.pi math.rad math.random math.randomseed math.sin math.sqrt math.tan
        math.tointeger math.type math.ult
        string string.byte string.char string.dump string.find string.format string.gmatch
        string.gsub string.len string.lower string.match string.pack string.packsize string.rep
        string.reverse string.sub string.unpack string.upper
        table table.clear table.concat table.freeze table.insert table.interceptall
        table.isfrozen table.move table.pack table.remove table.sort table.unpack
        utf8 utf8.char utf8.charpattern utf8.codepoint utf8.codes utf8.len utf8.offset
    ";

    #[test]
    fn each_vm_lends_exactly_its_allowlist() {
        let walked = probe(&format!("(function() {WALK} end)()"));
        let (added, gone) = against(&walked, CONFIG_ALLOWED, CONFIG_OPTIONAL);
        assert!(added.is_empty(), "the config VM lends, unlisted: {added:?}");
        assert!(
            gone.is_empty(),
            "listed, and the config VM no longer lends: {gone:?}"
        );
        let walked = project_probe(&format!("(function() {WALK} end)()"));
        let (added, gone) = against(&walked, PROJECT_ALLOWED, "");
        assert!(
            added.is_empty(),
            "the data-only VM lends, unlisted: {added:?}"
        );
        assert!(
            gone.is_empty(),
            "listed, and the data-only VM no longer lends: {gone:?}"
        );
        for refused in [
            "debug",
            "io",
            "package",
            "require",
            "dofile",
            "loadfile",
            "os.execute",
        ] {
            assert!(
                !CONFIG_ALLOWED
                    .split_whitespace()
                    .any(|name| name == refused)
            );
        }
        for refused in [
            "load",
            "print",
            "warn",
            "collectgarbage",
            "os",
            "magi.stream",
        ] {
            assert!(
                !PROJECT_ALLOWED
                    .split_whitespace()
                    .any(|name| name == refused)
            );
        }
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
    fn a_metatable_carrying_gc_is_refused_in_either_vm_and_any_other_is_taken() {
        let refused = "select(2, pcall(setmetatable, {}, { __gc = function() end }))";
        let kept = "(function() local t = {} \
                    return setmetatable(t, { __index = { x = 7 } }) == t and t.x end)()";
        for said in [probe(refused), project_probe(refused)] {
            assert!(said.contains("__gc"), "{said}");
        }
        assert_eq!(probe(kept), "7");
        assert_eq!(project_probe(kept), "7");
        assert_eq!(probe("debug"), "nil");
        let shared = "type(getmetatable(select(2, pcall(string.rep))))";
        assert_eq!(probe(shared), "nil");
        assert_eq!(project_probe(shared), "nil");
        assert_eq!(project_probe("type(getmetatable(''))"), "table");
    }

    #[test]
    fn print_and_warn_go_to_the_log_and_never_to_the_terminal() {
        if std::env::var_os("MAGI_PRINT_PROBE").is_some() {
            Engine::new()
                .run(
                    "print('printed', 1, nil, true) warn('warned ', 'twice')",
                    "plugin/noisy.lua",
                )
                .expect("runs");
            return;
        }
        let dir = magi_model::scratch::Scratch::new("magi-print", "log");
        let log = dir.join("debug.log");
        let probe = "sandbox::tests::print_and_warn_go_to_the_log_and_never_to_the_terminal";
        let ran = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", probe, "--nocapture"])
            .env("MAGI_PRINT_PROBE", "1")
            .env("MAGI_DEBUG_LOG", &log)
            .env_remove(magi_model::noted::FAMILY)
            .output()
            .expect("the probe");
        assert!(ran.status.success(), "{ran:?}");
        let terminal = [ran.stdout, ran.stderr].concat();
        let terminal = String::from_utf8_lossy(&terminal);
        assert!(
            !terminal.contains("printed") && !terminal.contains("warned"),
            "{terminal}"
        );
        let logged = std::fs::read_to_string(&log).expect("the log");
        assert!(
            logged.contains("lua print: printed\t1\tnil\ttrue"),
            "{logged}"
        );
        assert!(logged.contains("lua warn: warned \ttwice"), "{logged}");
    }

    #[test]
    fn load_refuses_a_precompiled_chunk_whatever_mode_it_names() {
        for chunk in ["string.dump(load('return 1'))", "'\\27Lua\\84\\0'"] {
            for mode in ["nil", "'b'", "'bt'", "'t'"] {
                let said = probe(&format!("select(2, pcall(load, {chunk}, 'chunk', {mode}))"));
                assert!(said.contains("precompiled"), "{chunk} as {mode}: {said}");
            }
        }
        assert_eq!(probe("load('return 41 + 1')()"), "42");
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
