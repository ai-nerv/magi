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
    /// Settings assigned onto the module, as JSON.
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// What a config said that magi did not keep, and why, in the order it said it.
    ///
    /// Two kinds end up here and both used to be silent. A declaration for something magi does
    /// not own — `magi.provider`, which is melchior's — and a setting too deeply nested to
    /// describe as JSON, which [`Engine::harvest`] cannot convert and used to simply skip. In
    /// both cases the config author wrote something that did nothing and nothing said so.
    pub unkept: Vec<String>,
    /// Files `magi.load` asked for, in the order it asked.
    ///
    /// Collected rather than run on the spot: running a chunk from inside a chunk is
    /// re-entrancy the VM does not offer, and a queue the host drains gives the same ordering
    /// with none of it. A file already asked for is not queued twice, so a diamond of loads
    /// terminates.
    pub loads: Vec<String>,
}

impl Config {
    /// A setting, if the config assigned one.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&serde_json::Value> {
        self.settings.get(name)
    }

    /// A setting as a string.
    #[must_use]
    pub fn string(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(serde_json::Value::as_str)
    }

    /// A setting as a boolean.
    #[must_use]
    pub fn boolean(&self, name: &str) -> Option<bool> {
        self.get(name).and_then(serde_json::Value::as_bool)
    }

    /// A setting as a number.
    ///
    /// Lua has one number type, so `2` and `2.0` are the same value written twice and both have
    /// to answer here — a config that says `2` and gets nothing would be right to call that a
    /// bug in magi.
    #[must_use]
    pub fn number(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(serde_json::Value::as_f64)
    }
}

/// The Lua VM, holding whatever the config has declared so far.
pub struct Engine {
    lua: Lua,
    config: Rc<RefCell<Config>>,
    /// What [`Engine::install`] itself put on the `magi` table.
    ///
    /// Captured rather than listed, so it cannot drift from what is actually installed. It is
    /// what lets [`Engine::harvest`] tell "a setting this config assigned and I could not keep"
    /// from "a primitive I put there myself" — `magi.stream` and `magi.json` are tables of
    /// functions, so they do not describe as JSON either, and complaining about them would mean
    /// complaining on every run of every config.
    installed: std::collections::HashSet<String>,
    /// The session's `Ops`, for [`crate::shell`]. Empty in every path but a real session.
    lent: crate::shell::Lent,
    /// Whether a tool's `run` is on the stack, which is the only time `magi.shell` answers.
    inside: crate::shell::Inside,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// A VM with the `magi` module installed and nothing declared.
    #[must_use]
    pub fn new() -> Self {
        let mut engine = Self {
            lua: Lua::full(),
            config: Rc::new(RefCell::new(Config::default())),
            lent: Rc::new(RefCell::new(None)),
            inside: Rc::new(std::cell::Cell::new(false)),
            installed: std::collections::HashSet::new(),
        };
        engine.install();
        // After install, so a removal cannot be undone by something the installer adds.
        crate::sandbox::apply(&mut engine.lua);
        engine
    }

    /// What the config has declared.
    #[must_use]
    pub fn config(&self) -> Config {
        self.config.borrow().clone()
    }

    /// Run one config file.
    ///
    /// Load-time raises are fatal and name the file: a config that did not finish has not said
    /// what it wanted, and applying half of it is worse than refusing.
    pub fn run_file(&mut self, path: &Path) -> Result<(), LuaError> {
        let source = std::fs::read_to_string(path).map_err(|source| LuaError::Io {
            file: path.display().to_string(),
            source,
        })?;
        self.run(&source, &path.display().to_string())
    }

    /// Run one config chunk.
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

    /// Install the `magi` global and its registrars.
    ///
    /// Settings are plain fields on the table, read back after the config runs rather than
    /// intercepted as they are written: a config may assign, read and re-assign its own
    /// settings, and only the value it finished with is the one it meant.
    fn install(&mut self) {
        let config = Rc::clone(&self.config);
        let mut mine = std::collections::HashSet::new();
        let lent = Rc::clone(&self.lent);
        let inside = Rc::clone(&self.inside);
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

            // The one way a config reaches another file. There is no auto-discovery behind it:
            // `init.lua` is the entry point, and what it does not name does not run.
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

            // Made here rather than left to the config, so `magi.ui.accent = 1` works without a
            // config having to write `magi.ui = {}` first. It is a plain settings table — nothing
            // registers into it — and it harvests as a nested object like any other, so a config
            // may also replace it wholesale.
            magi.set(ctx, "ui", Table::new(&ctx)).ok();

            // The socket primitive, so the family's clients run unchanged in this VM and magi can
            // dial oslo and hexe. Named twice: `magi.stream` for a client that knows this host,
            // `__stream` for one that does not.
            let stream = crate::stream::table(ctx);
            magi.set(ctx, "stream", stream).ok();
            // The lister a sibling's client prefers over shelling out. See `fs` for why `fs.dir`
            // is not offered alongside it.
            let fs = crate::fs::table(ctx);
            magi.set(ctx, "fs", fs).ok();
            // Running a command, through the same gate the shell peer goes through. Answers
            // only while a tool's `run` is on the stack -- see [`crate::shell`].
            let shell = crate::shell::callback(ctx, Rc::clone(&lent), Rc::clone(&inside));
            magi.set(ctx, "shell", shell).ok();
            // Every protocol description reads JSON payloads; lending one parser beats each
            // of them carrying its own.
            let json = crate::json::table(ctx);
            magi.set(ctx, "json", json).ok();
            ctx.set_global("__stream", stream);

            let tools = Table::new(&ctx);
            ctx.set_global(TOOLS, tools);

            // Tools register the same way protocols do, and for the same reason: a `run`
            // function cannot be described as data, so the VM keeps the whole declaration.
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

            // And watchers, for the third time and the same reason. This was a plain registrar
            // to begin with, alongside `provider` and `shell`, and it could not work: those
            // convert what they are handed to JSON, a `run` function does not survive that, and
            // every watcher was refused at load with "this table cannot be described".
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

            // The path of the binary that is running, so a config can name a peer magi ships
            // without hoping the right `magi` is on PATH. It is a multi-call binary: its own
            // peers are the same executable under another name, and `command = "magi"` finds
            // whichever copy the shell happens to see -- an older install, or none at all,
            // and the failure arrives as a broken pipe with nothing to read.
            if let Ok(exe) = std::env::current_exe() {
                let path = luna::String::from_slice(&ctx, exe.as_os_str().as_encoded_bytes());
                magi.set(ctx, "self", path).ok();
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

    /// Read the settings the config assigned, and forget the module.
    ///
    /// Called once after every file has run. Settings live as fields so a config can read its
    /// own back; harvesting them here is what keeps that true without a write barrier.
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
                    // Everything else that will not convert is a table nested past the bound, and
                    // dropping it in silence is how a setting a config plainly assigned came back
                    // as "not set". The bound stays — it is what stops a cycle becoming a stack
                    // overflow — and now it says so.
                    held.unkept.push(format!(
                        "magi.{name} was not kept: it nests deeper than magi will describe"
                    ));
                }
            }
        });
    }
}

/// Registrars that describe something magi does not own, and who does.
///
/// **All four stored what they were handed and nothing ever read it.** `magi.provider` was the
/// worst: melchior owns the model, so a provider declared in your own configuration went into a
/// map with no reader, and the only code that ever looked at that map was the check that refuses
/// a *project* file's providers. Declaring one worked exactly as well as not declaring one, and
/// nothing said so either way.
///
/// `magi.shell` was stranger still — the registrar was installed and then overwritten a few
/// lines below by the shell primitive of the same name, so calling it never reached this code at
/// all.
///
/// Kept as signposts rather than deleted outright. A configuration that says `magi.provider(…)`
/// today is wrong, and the two ways of being told so are a message naming melchior or
/// `attempt to call a nil value (field 'provider')`. The first is the one that helps.
const MOVED: &[(&str, &str, &str)] = &[
    ("provider", "melchior", "a provider is"),
    ("agent", "melchior", "sessions are"),
    ("shell", "casper", "running a command is"),
    ("mux", "hexe", "the multiplexer is"),
];

/// Raise a message into Lua, so `pcall` in a config sees a string.
fn raise<'gc>(ctx: luna::Context<'gc>, message: &str) -> luna::Error<'gc> {
    luna::Error::from_value(Value::String(luna::String::from_slice(
        &ctx,
        message.as_bytes(),
    )))
}

/// Where registered tool declarations live inside the VM.
const TOOLS: &str = "__magi_tools";

/// Where registered watchers live inside the VM.
///
/// A Lua table for the same reason [`TOOLS`] is one: what a watcher carries is a function, and
/// a function cannot be described as data. It was a plain registrar first, and the JSON round
/// trip that every registrar does dropped the function and refused the whole declaration.
const WATCHING: &str = "__magi_watching";

impl Engine {
    /// The tools a config registered, as `(name, declaration json)`.
    #[must_use]
    pub fn tools(&mut self) -> Vec<(String, serde_json::Value)> {
        let mut out = Vec::new();
        self.lua.enter(|ctx| {
            let Value::Table(tools) = ctx.get_global_value(TOOLS) else {
                return;
            };
            for (key, value) in tools.iter(ctx) {
                let Value::String(name) = key else { continue };
                // The `run` function cannot be described, so what comes back is the
                // declaration without it; the function stays in the VM where it belongs.
                if let Some(json) = crate::convert::declaration_from_lua(ctx, value, 0) {
                    out.push((String::from_utf8_lossy(name.as_bytes()).into_owned(), json));
                }
            }
        });
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Tell every registered watcher that a tool finished.
    ///
    /// Each in turn, each in `pcall`, and nothing it returns is read. A configuration that
    /// raises here costs itself that observation and nothing else — see the `Watch` trait for why
    /// that is the whole point of watching after the fact.
    ///
    /// One pass over the table rather than a call per name: the watchers live in the VM, and
    /// there is nothing Rust needs from them on the way past.
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

    /// Run a registered tool's `run` function.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        self.lua.enter(|ctx| {
            let value = crate::convert::lua_from_json(ctx, arguments);
            ctx.set_global("__magi_tool_args", value);
        });

        // Wrapped in `pcall` so a raise inside a tool is a failed call rather than a failed
        // turn: a config author's mistake must not cost the conversation.
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
        self.inside_a_tool().set(true);
        let ran = self.run(&source, "tool.lua");
        self.inside_a_tool().set(false);
        ran.ok()?;

        let mut out = None;
        self.lua.enter(|ctx| {
            out = crate::convert::json_from_lua(ctx, ctx.get_global_value("__magi_tool_result"), 0);
        });
        out.filter(|value| !value.is_null())
    }
}

impl Engine {
    /// Hand the VM the family's client libraries, as source.
    ///
    /// Read by Rust and passed in rather than opened by the config, because `io` is not
    /// reachable from a config and should not be: a tool needing one file is not a reason to
    /// give every config the ability to open any.
    ///
    /// `magi.clients.hexe` is then a string a tool loads with `load(...)`, which is exactly how
    /// the family says a sibling's client should be consumed.
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
}

impl Engine {
    /// Files `magi.load` has asked for and this has not yet handed back.
    ///
    /// Drained rather than read, so a caller can run what it is given, let those files ask for
    /// more, and come back for the rest until there is none.
    pub fn take_loads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.config.borrow_mut().loads)
    }
}

impl Engine {
    /// What `magi.load` has asked for so far, without taking it.
    #[must_use]
    pub fn peek_loads(&self) -> Vec<String> {
        self.config.borrow().loads.clone()
    }
}

impl Engine {
    /// Queue files as if a config had asked for them.
    ///
    /// The host's way of supplying a default set. It goes through the same queue `magi.load`
    /// writes to, so there is one path into the loader and one order to reason about.
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
    /// Lend this VM the session's `Ops`, so `magi.shell` has a seam to go through.
    ///
    /// Once, by the daemon, after the gate is built. Every other path — `magi tools`, a config
    /// being read, a test — lends nothing, and `magi.shell` says so rather than running.
    pub fn attach_ops(&mut self, ops: Rc<dyn magi_tools::Ops>) {
        *self.lent.borrow_mut() = Some(ops);
    }

    /// Whether a tool's `run` is on the stack, which is the only time `magi.shell` answers.
    ///
    /// A config file is read by the same VM. One that could spawn while it was being *read*
    /// would be a worse hole than the one `magi.shell` closes, so the window is exactly the
    /// call and nothing else.
    fn inside_a_tool(&self) -> &crate::shell::Inside {
        &self.inside
    }
}
