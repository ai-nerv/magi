//! The VM, and the config API it hands `init.lua`.

use crate::LuaError;
use crate::convert::json_from_lua;
use luna::{Callback, CallbackReturn, Closure, Executor, Lua, Table, Value};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// What a config declared, once every file has run.
#[derive(Debug, Default, Clone)]
pub struct Config {
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// What a config said that magi did not keep, and why, in the order it said it: a declaration
    /// for something magi does not own, or a setting too deeply nested to describe as JSON.
    pub unkept: Vec<String>,
    /// Files `magi.load` asked for, in the order it asked. Collected rather than run on the spot,
    /// since a file already asked for is not queued twice and a diamond of loads terminates.
    pub loads: Vec<String>,
}

impl Config {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&serde_json::Value> {
        self.settings.get(name)
    }

    #[must_use]
    pub fn string(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(serde_json::Value::as_str)
    }

    #[must_use]
    pub fn boolean(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(serde_json::Value::as_bool)
    }

    /// A setting as a number. Lua has one number type, so `2` and `2.0` both have to answer here.
    #[must_use]
    pub fn number(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(serde_json::Value::as_f64)
    }
}

pub struct Engine {
    lua: Lua,
    config: Rc<RefCell<Config>>,
    /// What [`Engine::install`] itself put on the `magi` table, captured rather than listed. It is
    /// what lets [`Engine::harvest`] tell an unkeepable setting from a primitive magi installed.
    installed: std::collections::HashSet<String>,
    /// The session's `Ops`, for `magi.fs.write`. Empty in every path but a real session.
    lent: crate::fs::Lent,
}

/// Which session this process is, and which balthasar holds it. Process-global because a magi is
/// one session; there is nothing to disambiguate.
static SESSION: std::sync::OnceLock<(String, Option<String>)> = std::sync::OnceLock::new();

/// Say which session this process is and where its balthasar listens. The socket matters as much
/// as the id: balthasar's client falls back to the newest socket in the directory, which is a
/// neighbour's as often as not. Called once; later calls are ignored rather than refused.
pub fn name_session(id: &str, socket: Option<&std::path::Path>) {
    let at = socket.map(|path| path.display().to_string());
    let _ = SESSION.set((id.to_owned(), at));
}

#[must_use]
pub fn session() -> Option<&'static str> {
    SESSION.get().map(|(id, _)| id.as_str())
}

#[must_use]
pub fn balthasar_at() -> Option<&'static str> {
    SESSION.get().and_then(|(_, at)| at.as_deref())
}

/// Which program fills each role here, as `ROLES.md` names them. Process-global for the reason
/// [`SESSION`] is: a magi fills each role once, for as long as it runs.
static ROLES: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

/// Say which program fills each role, so a VM built later can ask a sibling by the job rather than
/// by name. Called once; later calls are ignored rather than refused.
pub fn name_roles(roles: &[(String, String)]) {
    let _ = ROLES.set(roles.to_vec());
}

/// Every role and the program filling it, or nothing when nobody has said — a config test and a
/// worker a test built have no configuration to have said it.
#[must_use]
pub fn roles() -> &'static [(String, String)] {
    ROLES.get().map_or(&[], Vec::as_slice)
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        let mut engine = Self {
            lua: Lua::full(),
            config: Rc::new(RefCell::new(Config::default())),
            lent: Rc::new(RefCell::new(None)),
            installed: std::collections::HashSet::new(),
        };
        engine.install();
        // After install, so a removal cannot be undone by something the installer adds.
        crate::sandbox::apply(&mut engine.lua);
        engine
    }

    #[must_use]
    pub fn config(&self) -> Config {
        self.config.borrow().clone()
    }

    /// Run one config file. Load-time raises are fatal and name the file: applying half a config
    /// is worse than refusing it.
    pub fn run_file(&mut self, path: &Path) -> Result<(), LuaError> {
        let source = std::fs::read_to_string(path).map_err(|source| LuaError::Io {
            file: path.display().to_string(),
            source,
        })?;
        self.run(&source, &path.display().to_string())
    }

    pub fn run(&mut self, source: &str, chunk: &str) -> Result<(), LuaError> {
        let executor = self
            .lua
            .try_enter(|ctx| {
                let closure = Closure::load(ctx, Some(chunk), source.as_bytes())?;
                Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
            })
            .map_err(|e| LuaError::Syntax {
                file: chunk.to_owned(),
                message: e.to_string(),
            })?;

        self.lua
            .execute::<()>(&executor)
            .map_err(|e| LuaError::Runtime {
                file: chunk.to_owned(),
                message: e.to_string(),
            })
    }

    /// Install the `magi` global and its registrars. Settings are plain fields, read back after
    /// the config runs, so only the value it finished with is the one it meant.
    fn install(&mut self) {
        let config = Rc::clone(&self.config);
        let mut mine = std::collections::HashSet::new();
        let lent = Rc::clone(&self.lent);
        self.lua.enter(|ctx| {
            let magi = Table::new(&ctx);

            for (name, owner, what) in MOVED {
                let held = Rc::clone(&config);
                let callback = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                    let (id, _spec): (Value, Value) = stack.consume(ctx)?;
                    let id = match id {
                        Value::String(id) => String::from_utf8_lossy(id.as_bytes()).into_owned(),
                        _ => {
                            return Err(raise(
                                ctx,
                                &format!("magi.{name}: the first argument must be a name"),
                            ));
                        }
                    };
                    held.borrow_mut().unkept.push(format!(
                        "magi.{name}({id:?}) does nothing: {what} {owner}'s, and magi keeps no \
                         copy. Declare it in {owner}'s own configuration"
                    ));
                    stack.replace(ctx, ());
                    Ok(CallbackReturn::Return)
                });
                magi.set(ctx, *name, callback).ok();
            }

            // The one way a config reaches another file; `init.lua` is the entry point, and what
            // it does not name does not run.
            {
                let held = Rc::clone(&config);
                let load = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                    let path: Value = stack.consume(ctx)?;
                    let Value::String(path) = path else {
                        return Err(raise(ctx, "magi.load: expects a path"));
                    };
                    let path = String::from_utf8_lossy(path.as_bytes()).into_owned();
                    let mut held = held.borrow_mut();
                    if !held.loads.contains(&path) {
                        held.loads.push(path);
                    }
                    stack.replace(ctx, ());
                    Ok(CallbackReturn::Return)
                });
                magi.set(ctx, "load", load).ok();
            }

            // Made here so `magi.ui.accent = 1` works without a config writing `magi.ui = {}` first.
            magi.set(ctx, "ui", Table::new(&ctx)).ok();

            // The socket primitive, so the family's clients run unchanged here. Named twice:
            // `magi.stream` for a client that knows this host, `__stream` for one that does not.
            let stream = crate::stream::table(ctx);
            magi.set(ctx, "stream", stream).ok();
            // The lister a sibling's client prefers over shelling out.
            let fs = crate::fs::table(ctx, Rc::clone(&lent));
            magi.set(ctx, "fs", fs).ok();
            // magi runs no commands of its own — `magi.shell` is gone, and `MOVED` answers a config
            // that still calls it with "running a command is casper's". See `ROLES.md`.
            // Every protocol description reads JSON payloads; one lent parser beats each carrying one.
            let json = crate::json::table(ctx);
            magi.set(ctx, "json", json).ok();
            ctx.set_global("__stream", stream);

            let tools = Table::new(&ctx);
            ctx.set_global(TOOLS, tools);

            // Tools register the way protocols do: a `run` function cannot be described as data,
            // so the VM keeps the whole declaration.
            let tool = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                let (name, spec): (Value, Value) = stack.consume(ctx)?;
                let (Value::String(name), Value::Table(_)) = (name, spec) else {
                    return Err(raise(ctx, "magi.tool(name, spec): a name and a table"));
                };
                if let Value::Table(tools) = ctx.get_global_value(TOOLS) {
                    tools.set(ctx, name, spec).ok();
                }
                stack.replace(ctx, ());
                Ok(CallbackReturn::Return)
            });
            magi.set(ctx, "tool", tool).ok();

            // And watchers, for the same reason: a plain registrar converts to JSON, which drops
            // the function and refuses the declaration.
            let watching = Table::new(&ctx);
            ctx.set_global(WATCHING, watching);
            let watch = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                let (name, spec): (Value, Value) = stack.consume(ctx)?;
                let (Value::String(name), Value::Table(_)) = (name, spec) else {
                    return Err(raise(ctx, "magi.watch(name, spec): a name and a table"));
                };
                if let Value::Table(watching) = ctx.get_global_value(WATCHING) {
                    watching.set(ctx, name, spec).ok();
                }
                stack.replace(ctx, ());
                Ok(CallbackReturn::Return)
            });
            magi.set(ctx, "watch", watch).ok();

            // The path of the running binary, so a config can name a peer magi ships. It is a
            // multi-call binary, and `command = "magi"` finds whichever copy the shell sees.
            if let Ok(exe) = std::env::current_exe() {
                let path = luna::String::from_slice(&ctx, exe.as_os_str().as_encoded_bytes());
                magi.set(ctx, "self", path).ok();
            }

            // Which session this is, for a tool that has to name it. Absent in a VM nobody named a
            // session for, which is what a config test and `magi tools` have.
            if let Some(id) = session() {
                let id = luna::String::from_slice(&ctx, id.as_bytes());
                magi.set(ctx, "session", id).ok();
            }
            // And where its balthasar listens, so a memory tool asks this session's rather than
            // whichever socket is newest.
            if let Some(at) = balthasar_at() {
                let at = luna::String::from_slice(&ctx, at.as_bytes());
                magi.set(ctx, "balthasar_at", at).ok();
            }
            // And which program fills each role, so a tool description names the job.
            if !roles().is_empty() {
                let table = Table::new(&ctx);
                for (role, program) in roles() {
                    let program = luna::String::from_slice(&ctx, program.as_bytes());
                    table.set(ctx, role.as_str(), program).ok();
                }
                magi.set(ctx, "roles", table).ok();
            }

            for (key, _) in magi.iter(ctx) {
                if let Value::String(name) = key {
                    mine.insert(String::from_utf8_lossy(name.as_bytes()).into_owned());
                }
            }
            ctx.set_global("magi", magi);
        });
        self.installed = mine;
    }

    /// Read the settings the config assigned, and forget the module. Settings live as fields so a
    /// config can read its own back; harvesting here keeps that true without a write barrier.
    pub fn harvest(&mut self) {
        let config = Rc::clone(&self.config);
        self.lua.enter(|ctx| {
            let Value::Table(magi) = ctx.get_global_value("magi") else {
                return;
            };
            let mut held = config.borrow_mut();
            for (key, value) in magi.iter(ctx) {
                let Value::String(name) = key else { continue };
                let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
                // A registrar is a function and cannot be described; skipping it is what makes
                // "every other field is a setting" work without a list to keep in step.
                if let Some(json) = json_from_lua(ctx, value, 0) {
                    held.settings.insert(name, json);
                } else if !self.installed.contains(&name) {
                    // Everything else that will not convert is a table nested past the bound. The
                    // bound stays — it is what stops a cycle becoming a stack overflow.
                    held.unkept.push(format!(
                        "magi.{name} was not kept: it nests deeper than magi will describe"
                    ));
                }
            }
        });
    }
}

/// Registrars that describe something magi does not own, and who does. Kept as signposts rather
/// than deleted: a configuration that says `magi.provider(…)` today is wrong, and a message naming
/// melchior helps where `attempt to call a nil value` does not.
const MOVED: &[(&str, &str, &str)] = &[
    ("provider", "melchior", "a provider is"),
    ("agent", "melchior", "sessions are"),
    ("shell", "casper", "running a command is"),
    ("mux", "hexe", "the multiplexer is"),
];

fn raise<'gc>(ctx: luna::Context<'gc>, message: &str) -> luna::Error<'gc> {
    luna::Error::from_value(Value::String(luna::String::from_slice(
        &ctx,
        message.as_bytes(),
    )))
}

/// Where registered tool declarations live inside the VM.
const TOOLS: &str = "__magi_tools";

/// Where registered watchers live inside the VM. A Lua table for the same reason [`TOOLS`] is one:
/// a watcher carries a function, and a function cannot be described as data.
const WATCHING: &str = "__magi_watching";

impl Engine {
    #[must_use]
    pub fn tools(&mut self) -> Vec<(String, serde_json::Value)> {
        let mut out = Vec::new();
        self.lua.enter(|ctx| {
            let Value::Table(tools) = ctx.get_global_value(TOOLS) else {
                return;
            };
            for (key, value) in tools.iter(ctx) {
                let Value::String(name) = key else { continue };
                // The `run` function cannot be described, so what comes back is the declaration
                // without it; the function stays in the VM.
                if let Some(json) = crate::convert::declaration_from_lua(ctx, value, 0) {
                    out.push((String::from_utf8_lossy(name.as_bytes()).into_owned(), json));
                }
            }
        });
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Tell every registered watcher that a tool finished. Each in turn, each in `pcall`, and
    /// nothing it returns is read: a configuration that raises here costs itself that observation.
    pub fn call_watchers(&mut self, event: &serde_json::Value) {
        let mut any = false;
        self.lua.enter(|ctx| {
            if let Value::Table(watching) = ctx.get_global_value(WATCHING) {
                any = watching.iter(ctx).next().is_some();
            }
            if any {
                let value = crate::convert::lua_from_json(ctx, event);
                ctx.set_global("__magi_watch_event", value);
            }
        });
        if !any {
            return;
        }
        let source = format!(
            "for _, w in pairs({WATCHING}) do\n\
             \x20 if type(w) == \"table\" and w.run then pcall(w.run, __magi_watch_event) end\n\
             end"
        );
        let _ = self.run(&source, "watch.lua");
    }

    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        self.lua.enter(|ctx| {
            let value = crate::convert::lua_from_json(ctx, arguments);
            ctx.set_global("__magi_tool_args", value);
        });

        // Wrapped in `pcall` so a raise inside a tool is a failed call rather than a failed turn.
        let source = format!(
            "local spec = {TOOLS} and {TOOLS}[{name:?}]\n\
             local fn = spec and spec.run\n\
             if not fn then __magi_tool_result = nil return end\n\
             local ok, answer = pcall(fn, __magi_tool_args)\n\
             if not ok then\n\
               __magi_tool_result = {{ content = tostring(answer), is_error = true }}\n\
             elseif type(answer) == \"string\" then\n\
               __magi_tool_result = {{ content = answer, is_error = false }}\n\
             else\n\
               __magi_tool_result = answer\n\
             end"
        );
        let ran = self.run(&source, "tool.lua");
        ran.ok()?;

        let mut out = None;
        self.lua.enter(|ctx| {
            out = crate::convert::json_from_lua(ctx, ctx.get_global_value("__magi_tool_result"), 0);
        });
        out.filter(|value| !value.is_null())
    }
}

impl Engine {
    /// Hand the VM the family's client libraries, as source. Read by Rust and passed in rather
    /// than opened by the config, because `io` is not reachable from a config and should not be.
    pub fn install_clients(&mut self, clients: &[(String, String)]) {
        self.lua.enter(|ctx| {
            let table = Table::new(&ctx);
            for (name, source) in clients {
                let source = luna::String::from_slice(&ctx, source.as_bytes());
                table.set(ctx, name.as_str(), source).ok();
            }
            if let Value::Table(magi) = ctx.get_global_value("magi") {
                magi.set(ctx, "clients", table).ok();
            }
        });
    }

    /// Hand the VM which program fills each role. Separate from [`name_roles`], which reaches only
    /// the VMs built after it: the VM that reads the configuration is the one that learns the roles,
    /// so it is already running when they are known.
    pub fn install_roles(&mut self, roles: &[(String, String)]) {
        self.lua.enter(|ctx| {
            let table = Table::new(&ctx);
            for (role, program) in roles {
                let program = luna::String::from_slice(&ctx, program.as_bytes());
                table.set(ctx, role.as_str(), program).ok();
            }
            if let Value::Table(magi) = ctx.get_global_value("magi") {
                magi.set(ctx, "roles", table).ok();
            }
        });
    }

    /// One string setting as the config has left it, without harvesting. For the settings that
    /// decide what is loaded next: which program fills a role is needed before the tool
    /// descriptions that ask that program anything are run.
    #[must_use]
    pub fn setting(&mut self, name: &str) -> Option<String> {
        let mut out = None;
        self.lua.enter(|ctx| {
            if let Value::Table(magi) = ctx.get_global_value("magi")
                && let Value::String(text) = magi.get_value(ctx, name)
            {
                out = Some(String::from_utf8_lossy(text.as_bytes()).into_owned());
            }
        });
        out
    }
}

impl Engine {
    /// Files `magi.load` has asked for and this has not yet handed back; drained rather than read.
    pub fn take_loads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.config.borrow_mut().loads)
    }
}

impl Engine {
    #[must_use]
    pub fn peek_loads(&self) -> Vec<String> {
        self.config.borrow().loads.clone()
    }
}

impl Engine {
    /// Queue files as if a config had asked for them, through the same queue `magi.load` writes to.
    pub fn load_all(&mut self, paths: impl IntoIterator<Item = String>) {
        let mut held = self.config.borrow_mut();
        for path in paths {
            if !held.loads.contains(&path) {
                held.loads.push(path);
            }
        }
    }
}

impl Engine {
    /// Lend this VM the session's `Ops`, so `magi.fs.write` has a seam. Once, by the daemon; every
    /// other path lends nothing and `magi.fs.write` says so rather than writing.
    pub fn attach_ops(&mut self, ops: Rc<dyn magi_tools::Ops>) {
        *self.lent.borrow_mut() = Some(ops);
    }
}

#[cfg(test)]
#[path = "engine/naming.rs"]
mod naming;
