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
    /// Everything handed to a registrar, keyed by registrar then by identity.
    pub registered: Registered,
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

    /// Everything handed to one registrar, in declaration order.
    #[must_use]
    pub fn all(&self, registrar: &str) -> Vec<(&str, &serde_json::Value)> {
        self.registered
            .order
            .iter()
            .filter(|(kind, _)| kind == registrar)
            .filter_map(|(kind, id)| {
                self.registered
                    .entries
                    .get(&(kind.clone(), id.clone()))
                    .map(|value| (id.as_str(), value))
            })
            .collect()
    }
}

/// Declarations handed to registrars.
///
/// Keyed by `(registrar, identity)` so re-registering replaces rather than appends — the map
/// form of rule 2. A config that loops over a directory of machines and declares one provider
/// per file is then idempotent, which matters because configs get re-read.
#[derive(Debug, Default, Clone)]
pub struct Registered {
    entries: std::collections::HashMap<(String, String), serde_json::Value>,
    /// Declaration order, so a model picker's list does not reshuffle between runs.
    order: Vec<(String, String)>,
}

impl Registered {
    fn insert(&mut self, registrar: &str, id: &str, value: serde_json::Value) {
        let key = (registrar.to_owned(), id.to_owned());
        if !self.entries.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.entries.insert(key, value);
    }
}

/// The Lua VM, holding whatever the config has declared so far.
pub struct Engine {
    lua: Lua,
    config: Rc<RefCell<Config>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// A VM with the `axum` module installed and nothing declared.
    #[must_use]
    pub fn new() -> Self {
        let mut engine = Self {
            lua: Lua::full(),
            config: Rc::new(RefCell::new(Config::default())),
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

    /// Install the `axum` global and its registrars.
    ///
    /// Settings are plain fields on the table, read back after the config runs rather than
    /// intercepted as they are written: a config may assign, read and re-assign its own
    /// settings, and only the value it finished with is the one it meant.
    fn install(&mut self) {
        let config = Rc::clone(&self.config);
        self.lua.enter(|ctx| {
            let axum = Table::new(&ctx);

            for registrar in REGISTRARS {
                let held = Rc::clone(&config);
                let name = *registrar;
                let callback = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                    let (id, spec): (Value, Value) = stack.consume(ctx)?;
                    let Value::String(id) = id else {
                        return Err(raise(
                            ctx,
                            &format!("axum.{name}: the first argument must be a name"),
                        ));
                    };
                    let id = String::from_utf8_lossy(id.as_bytes()).into_owned();

                    let Some(value) = json_from_lua(ctx, spec, 0) else {
                        return Err(raise(
                            ctx,
                            &format!("axum.{name}({id}): this table cannot be described"),
                        ));
                    };
                    held.borrow_mut().registered.insert(name, &id, value);
                    stack.replace(ctx, ());
                    Ok(CallbackReturn::Return)
                });
                axum.set(ctx, *registrar, callback).ok();
            }

            // The socket primitive, so the family's stubs run unchanged in this VM and axum can
            // dial oslo and hexe. Named twice: `axum.stream` for a stub that knows this host,
            // `__stream` for one that does not.
            let stream = crate::stream::table(ctx);
            axum.set(ctx, "stream", stream).ok();
            // The lister a sibling's stub prefers over shelling out. See `fs` for why `fs.dir`
            // is not offered alongside it.
            let fs = crate::fs::table(ctx);
            axum.set(ctx, "fs", fs).ok();
            // Every protocol description reads JSON payloads; lending one parser beats each
            // of them carrying its own.
            let json = crate::json::table(ctx);
            axum.set(ctx, "json", json).ok();
            ctx.set_global("__stream", stream);

            // Protocols are registered differently from everything else: what they carry is
            // functions, and a function cannot be described as data. The VM keeps them under
            // `__axum_apis` and Rust keeps only their names.
            let apis = Table::new(&ctx);
            ctx.set_global(APIS, apis);
            let tools = Table::new(&ctx);
            ctx.set_global(TOOLS, tools);
            let api = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                let (name, spec): (Value, Value) = stack.consume(ctx)?;
                let (Value::String(name), Value::Table(_)) = (name, spec) else {
                    return Err(raise(ctx, "axum.api(name, spec): a name and a table"));
                };
                if let Value::Table(apis) = ctx.get_global_value(APIS) {
                    apis.set(ctx, name, spec).ok();
                }
                stack.replace(ctx, ());
                Ok(CallbackReturn::Return)
            });
            axum.set(ctx, "api", api).ok();

            // Tools register the same way protocols do, and for the same reason: a `run`
            // function cannot be described as data, so the VM keeps the whole declaration.
            let tool = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                let (name, spec): (Value, Value) = stack.consume(ctx)?;
                let (Value::String(name), Value::Table(_)) = (name, spec) else {
                    return Err(raise(ctx, "axum.tool(name, spec): a name and a table"));
                };
                if let Value::Table(tools) = ctx.get_global_value(TOOLS) {
                    tools.set(ctx, name, spec).ok();
                }
                stack.replace(ctx, ());
                Ok(CallbackReturn::Return)
            });
            axum.set(ctx, "tool", tool).ok();

            ctx.set_global("axum", axum);
        });
    }

    /// Read the settings the config assigned, and forget the module.
    ///
    /// Called once after every file has run. Settings live as fields so a config can read its
    /// own back; harvesting them here is what keeps that true without a write barrier.
    pub fn harvest(&mut self) {
        let config = Rc::clone(&self.config);
        self.lua.enter(|ctx| {
            let Value::Table(axum) = ctx.get_global_value("axum") else {
                return;
            };
            let mut held = config.borrow_mut();
            for (key, value) in axum.iter(ctx) {
                let Value::String(name) = key else { continue };
                let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
                // A registrar is a function and cannot be described; skipping it is what makes
                // "every other field is a setting" work without a list to keep in step.
                if let Some(json) = json_from_lua(ctx, value, 0) {
                    held.settings.insert(name, json);
                }
            }
        });
    }
}

/// The registrars the `axum` module offers.
///
/// Named for the thing being described, never for when it happens. Adding one here is the only
/// way a config gains a new kind of declaration, which keeps the surface enumerable.
const REGISTRARS: &[&str] = &["provider", "agent", "shell", "mux"];

/// Raise a message into Lua, so `pcall` in a config sees a string.
fn raise<'gc>(ctx: luna::Context<'gc>, message: &str) -> luna::Error<'gc> {
    luna::Error::from_value(Value::String(luna::String::from_slice(
        &ctx,
        message.as_bytes(),
    )))
}

/// Where registered protocol descriptions live inside the VM.
///
/// A Lua table rather than a Rust map, because what is registered is *functions*, and a
/// function cannot cross the boundary. The VM keeps them; Rust keeps only their names.
const APIS: &str = "__axum_apis";

impl Engine {
    /// The protocols a config registered.
    #[must_use]
    pub fn apis(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        self.lua.enter(|ctx| {
            if let Value::Table(apis) = ctx.get_global_value(APIS) {
                for (key, _) in apis.iter(ctx) {
                    if let Value::String(name) = key {
                        out.push(String::from_utf8_lossy(name.as_bytes()).into_owned());
                    }
                }
            }
        });
        out.sort();
        out
    }

    /// Call one function of a registered protocol.
    ///
    /// Arguments go in as JSON and the answer comes back as JSON, so the collector lifetime
    /// never leaves this crate. `None` means the protocol, the function, or the call itself did
    /// not produce a value — all of which the caller treats the same way: the protocol cannot
    /// answer, so the turn fails rather than proceeding on a guess.
    pub fn call_api(
        &mut self,
        api: &str,
        method: &str,
        args: &[serde_json::Value],
    ) -> Option<serde_json::Value> {
        let args = serde_json::Value::Array(args.to_vec());
        self.lua.enter(|ctx| {
            let value = crate::convert::lua_from_json(ctx, &args);
            ctx.set_global("__axum_args", value);
        });

        let source = format!(
            "local api = {APIS} and {APIS}[{api:?}]\n\
             local fn = api and api[{method:?}]\n\
             if fn then __axum_result = fn(table.unpack(__axum_args)) \
             else __axum_result = nil end"
        );
        self.run(&source, "api.lua").ok()?;

        let mut out = None;
        self.lua.enter(|ctx| {
            out = crate::convert::json_from_lua(ctx, ctx.get_global_value("__axum_result"), 0);
        });
        out.filter(|value| !value.is_null())
    }
}

/// Where registered tool declarations live inside the VM.
const TOOLS: &str = "__axum_tools";

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

    /// Run a registered tool's `run` function.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        self.lua.enter(|ctx| {
            let value = crate::convert::lua_from_json(ctx, arguments);
            ctx.set_global("__axum_tool_args", value);
        });

        // Wrapped in `pcall` so a raise inside a tool is a failed call rather than a failed
        // turn: a config author's mistake must not cost the conversation.
        let source = format!(
            "local spec = {TOOLS} and {TOOLS}[{name:?}]\n\
             local fn = spec and spec.run\n\
             if not fn then __axum_tool_result = nil return end\n\
             local ok, answer = pcall(fn, __axum_tool_args)\n\
             if not ok then\n\
               __axum_tool_result = {{ content = tostring(answer), is_error = true }}\n\
             elseif type(answer) == \"string\" then\n\
               __axum_tool_result = {{ content = answer, is_error = false }}\n\
             else\n\
               __axum_tool_result = answer\n\
             end"
        );
        self.run(&source, "tool.lua").ok()?;

        let mut out = None;
        self.lua.enter(|ctx| {
            out = crate::convert::json_from_lua(ctx, ctx.get_global_value("__axum_tool_result"), 0);
        });
        out.filter(|value| !value.is_null())
    }
}

impl Engine {
    /// Hand the VM the family's client stubs, as source.
    ///
    /// Read by Rust and passed in rather than opened by the config, because `io` is not
    /// reachable from a config and should not be: a tool needing one file is not a reason to
    /// give every config the ability to open any.
    ///
    /// `axum.stubs.hexe` is then a string a tool loads with `load(...)`, which is exactly how
    /// the family says a sibling's stub should be consumed.
    pub fn install_stubs(&mut self, stubs: &[(String, String)]) {
        self.lua.enter(|ctx| {
            let table = Table::new(&ctx);
            for (name, source) in stubs {
                let source = luna::String::from_slice(&ctx, source.as_bytes());
                table.set(ctx, name.as_str(), source).ok();
            }
            if let Value::Table(axum) = ctx.get_global_value("axum") {
                axum.set(ctx, "stubs", table).ok();
            }
        });
    }
}
