//! Tools declared in a config file.
//!
//! `axon.tool(name, spec)` registers one. The spec says what it does, what arguments it takes,
//! and — the part that matters — **how it is reached**:
//!
//! ```lua
//! axon.tool("hexe", {
//!   description = "…", parameters = { … },
//!   transport = { kind = "lua" },
//!   run = function(args, ops) … end,
//! })
//!
//! axon.tool("bash", {
//!   description = "…", parameters = { … },
//!   transport = { kind = "process", command = "axon", args = { "ext", "shell" } },
//! })
//! ```
//!
//! Transport is a property of a declaration rather than a second registry, so adding a way to
//! reach a tool never adds a way to run one: everything lands in [`axon_tools::Registry`] and
//! the turn loop cannot tell them apart.

use crate::Engine;
use axon_tools::{Cancel, Ops, Output, Tool};
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

/// How a declared tool is reached.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Transport {
    /// A Lua function in the worker's VM.
    ///
    /// No process, no serialisation. It gets [`Ops`] — read and write, path-checked — and the
    /// VM's own natives: sockets, JSON, directory listing. Deliberately **not** a shell: if a
    /// tool needs to run commands it is a process, and it is isolated because it is one.
    Lua,
    /// A peer in its own process, spoken to over the wire.
    ///
    /// Any language, crash-isolated, and later sandboxable. What a tool that needs to outlive
    /// one call, hold a working directory, or be untrusted should be.
    Process {
        /// The program to run.
        command: String,
        /// Its arguments.
        #[serde(default, deserialize_with = "lua_list")]
        args: Vec<String>,
        /// Environment for this peer, beside what every process axon starts already gets.
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
    },
    /// An ordinary program axon runs, with arguments built from the call.
    ///
    /// Not a peer: the child is any unix tool and axon reads what it printed. This is how a config
    /// declares `grep`, `find` or `jq` without a peer to write or a shell string to quote. There is
    /// no shell -- see [`axon_tools::command::render`] for what an argument is and is not.
    Command {
        /// The program to run.
        command: String,
        /// Its arguments, each a literal or `{name}` naming a declared property.
        #[serde(default, deserialize_with = "lua_list")]
        args: Vec<String>,
        /// Environment for this program, beside what every process axon starts already gets.
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
        /// Seconds it may run before it is killed.
        #[serde(default)]
        timeout: Option<u64>,
    },
}

/// A tool as a config declared it.
#[derive(Debug, Clone, Deserialize)]
pub struct Declaration {
    /// What it does, in the model's terms.
    #[serde(default)]
    pub description: String,
    /// JSON Schema for its arguments.
    #[serde(default = "empty_object")]
    pub parameters: serde_json::Value,
    /// How it is reached.
    pub transport: Transport,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({ "type": "object" })
}

/// A tool whose body is a Lua function.
pub struct LuaTool {
    engine: Rc<RefCell<Engine>>,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

impl LuaTool {
    /// Build one from a declaration the VM already holds.
    #[must_use]
    pub fn new(engine: Rc<RefCell<Engine>>, name: &str, declaration: &Declaration) -> Self {
        Self {
            engine,
            name: name.to_owned(),
            description: declaration.description.clone(),
            parameters: declaration.parameters.clone(),
        }
    }
}

impl Tool for LuaTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    // A Lua body runs to completion inside the VM, so there is no point between entering it
    // and leaving it at which an interrupt could be noticed. Honesty about that is better than
    // a check that could never fire.
    fn run(&self, arguments: &serde_json::Value, _ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let answer = self.engine.borrow_mut().call_tool(&self.name, arguments);
        match answer {
            Some(value) => Output {
                content: value
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                is_error: value
                    .get("is_error")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            },
            // A description that raised, returned nothing, or has no `run` at all. Reported as
            // a result rather than a fault: the model asked for it and needs to be told.
            None => Output::error(format!("the tool {:?} did not answer", self.name)),
        }
    }
}

/// Build every declared tool into one registry, on top of the floor.
///
/// Both transports land here and the registry cannot tell them apart — which is the whole
/// design. A declaration that will not parse is skipped with a reason on stderr rather than
/// failing the daemon: one broken tool should cost you that tool, not the session.
pub fn install(
    engine: Rc<RefCell<Engine>>,
    registry: &mut axon_tools::Registry,
    environ: &std::collections::BTreeMap<String, String>,
) {
    // Installed once, whether or not anything is watching. A registrar with nothing in it
    // costs one empty lookup per tool call, and wiring it conditionally would mean a config
    // that adds a watcher after startup silently never fires.
    registry.watch(Box::new(LuaWatch::new(Rc::clone(&engine))));

    let declared = engine.borrow_mut().tools();
    for (name, spec) in declared {
        let declaration: Declaration = match serde_json::from_value(spec) {
            Ok(declaration) => declaration,
            Err(why) => {
                eprintln!("axon: the tool {name:?} was not registered: {why}");
                continue;
            }
        };
        match &declaration.transport {
            Transport::Lua => {
                registry.register(Box::new(LuaTool::new(
                    Rc::clone(&engine),
                    &name,
                    &declaration,
                )));
            }
            Transport::Command {
                command,
                args,
                env,
                timeout,
            } => {
                let tool = axon_tools::command::CommandTool::new(
                    &name,
                    &declaration.description,
                    declaration.parameters.clone(),
                    command,
                    args.clone(),
                )
                .with_env(
                    environ
                        .iter()
                        .chain(env)
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                );
                let tool = match timeout {
                    Some(seconds) => tool.with_timeout(*seconds),
                    None => tool,
                };
                // A placeholder naming a property the schema does not declare can never be
                // filled, so the argument silently vanishes at every call. Caught here, where
                // there is somebody to tell, rather than at the call where there is not.
                if let Some(unknown) = undeclared(&tool, &declaration.parameters) {
                    eprintln!(
                        "axon: the tool {name:?} was not registered: its arguments name {unknown:?}, which it does not declare"
                    );
                    continue;
                }
                registry.register(Box::new(tool));
            }
            Transport::Process { command, args, env } => {
                registry.register(Box::new(
                    axon_tools::process::ProcessTool::new(
                        &name,
                        &declaration.description,
                        declaration.parameters.clone(),
                        command,
                        args.clone(),
                    )
                    .with_env(
                        environ
                            .iter()
                            .chain(env)
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    ),
                ));
            }
        }
    }
}

/// A list that may arrive as an empty table.
///
/// Lua has one table type, so `{}` is both an empty array and an empty object and there is no
/// way to tell which was meant. Everything above this reads it as an object, which would make
/// `args = {}` — the ordinary way to say "no arguments" — a type error.
fn lua_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Array(items) => Ok(items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()),
        serde_json::Value::Object(fields) if fields.is_empty() => Ok(Vec::new()),
        serde_json::Value::Null => Ok(Vec::new()),
        other => Err(serde::de::Error::custom(format!(
            "expected a list, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_tools::Registry;
    use axon_tools::ops::Real;

    /// Run a config chunk and build what it declared.
    fn built(source: &str) -> (Registry, Rc<RefCell<Engine>>) {
        let mut engine = Engine::new();
        engine
            .run(source, "tools.lua")
            .expect("the config must run");
        let engine = Rc::new(RefCell::new(engine));
        let mut registry = Registry::new();
        axon_tools::builtin::install(&mut registry);
        install(Rc::clone(&engine), &mut registry, &Default::default());
        (registry, engine)
    }

    fn ops() -> Real {
        Real::new(std::env::temp_dir())
    }

    const LUA_TOOL: &str = r#"
        axon.tool("echo", {
          description = "Say it back.",
          parameters = { type = "object" },
          transport = { kind = "lua" },
          run = function(args) return "you said " .. tostring(args.text) end,
        })
    "#;

    #[test]
    fn a_lua_tool_runs_in_the_vm() {
        let (registry, _) = built(LUA_TOOL);
        let output = registry.call(
            "echo",
            &serde_json::json!({ "text": "hi" }),
            &ops(),
            &axon_tools::Uncancelled,
        );
        assert_eq!(output.content, "you said hi");
        assert!(!output.is_error);
    }

    #[test]
    fn a_string_return_is_a_successful_result() {
        // A config author should not have to build a table to say the ordinary thing.
        let (registry, _) = built(LUA_TOOL);
        assert!(
            !registry
                .call(
                    "echo",
                    &serde_json::json!({}),
                    &ops(),
                    &axon_tools::Uncancelled
                )
                .is_error
        );
    }

    #[test]
    fn a_lua_tool_that_raises_fails_the_call_not_the_turn() {
        let (registry, _) = built(
            r#"
            axon.tool("boom", {
              description = "Always raises.",
              transport = { kind = "lua" },
              run = function() error("deliberate") end,
            })
            "#,
        );
        let output = registry.call(
            "boom",
            &serde_json::json!({}),
            &ops(),
            &axon_tools::Uncancelled,
        );
        assert!(output.is_error);
        assert!(output.content.contains("deliberate"), "{}", output.content);
    }

    #[test]
    fn a_tool_may_report_a_failure_the_model_should_read() {
        let (registry, _) = built(
            r#"
            axon.tool("nope", {
              transport = { kind = "lua" },
              run = function() return { content = "no such thing", is_error = true } end,
            })
            "#,
        );
        let output = registry.call(
            "nope",
            &serde_json::json!({}),
            &ops(),
            &axon_tools::Uncancelled,
        );
        assert!(output.is_error);
        assert_eq!(output.content, "no such thing");
    }

    #[test]
    fn both_transports_land_in_one_registry() {
        let (registry, _) = built(
            r#"
            axon.tool("a-lua", { transport = { kind = "lua" }, run = function() return "x" end })
            axon.tool("a-process", {
              transport = { kind = "process", command = "true", args = {} },
            })
            "#,
        );
        // The floor plus both declarations, and nothing distinguishes them from outside.
        assert_eq!(registry.len(), 5);
        for name in ["read", "write", "edit", "a-lua", "a-process"] {
            assert!(registry.get(name).is_some(), "{name} is missing");
        }
    }

    #[test]
    fn a_declaration_can_replace_a_builtin() {
        let (registry, _) = built(
            r#"
            axon.tool("read", {
              description = "Mine instead.",
              transport = { kind = "lua" },
              run = function() return "mine" end,
            })
            "#,
        );
        assert_eq!(registry.len(), 3, "replaced, not added");
        assert_eq!(
            registry
                .call(
                    "read",
                    &serde_json::json!({}),
                    &ops(),
                    &axon_tools::Uncancelled
                )
                .content,
            "mine"
        );
    }

    #[test]
    fn a_malformed_declaration_costs_only_that_tool() {
        let (registry, _) = built(
            r#"
            axon.tool("broken", { transport = { kind = "carrier-pigeon" } })
            axon.tool("fine", { transport = { kind = "lua" }, run = function() return "ok" end })
            "#,
        );
        assert!(registry.get("broken").is_none());
        assert!(registry.get("fine").is_some(), "the session survives it");
    }

    #[test]
    fn declarations_reach_the_provider_with_their_schemas() {
        let (registry, _) = built(LUA_TOOL);
        let declared = registry.declarations();
        let echo = declared.iter().find(|t| t.name == "echo").expect("echo");
        assert_eq!(echo.description, "Say it back.");
        assert_eq!(echo.parameters["type"], "object");
    }
}

/// A placeholder in a command's arguments that its schema never declares.
///
/// `{limit}` against a schema with no `limit` property can never be filled, so the argument
/// disappears from every call and the tool quietly runs unbounded. The declaration is wrong, and
/// the config author is the only one who can fix it.
fn undeclared(
    tool: &axon_tools::command::CommandTool,
    parameters: &serde_json::Value,
) -> Option<String> {
    let declared = parameters.get("properties").and_then(|p| p.as_object());
    tool.placeholders()
        .into_iter()
        .find(|name| !declared.is_some_and(|properties| properties.contains_key(name)))
}

/// A config can declare a tool that is an ordinary program.
#[cfg(test)]
mod command_transport {
    use super::*;

    /// The transport a declaration parses into.
    fn transport(lua: &str) -> Result<Transport, String> {
        let mut engine = Engine::new();
        engine.run(lua, "test.lua").map_err(|e| e.to_string())?;
        engine.harvest();
        let (_, spec) = engine
            .tools()
            .into_iter()
            .next()
            .ok_or("nothing was declared")?;
        let declaration: Declaration =
            serde_json::from_value(spec).map_err(|why| why.to_string())?;
        Ok(declaration.transport)
    }

    #[test]
    fn a_command_declaration_parses() {
        let parsed = transport(
            r#"axon.tool("say", {
                 description = "prints",
                 parameters = { type = "object" },
                 transport = { kind = "command", command = "echo", args = { "hi" } },
               })"#,
        )
        .expect("it parses");
        assert_eq!(
            parsed,
            Transport::Command {
                command: "echo".to_owned(),
                args: vec!["hi".to_owned()],
                env: std::collections::BTreeMap::new(),
                timeout: None,
            }
        );
    }

    #[test]
    fn a_timeout_is_carried() {
        let parsed = transport(
            r#"axon.tool("slow", {
                 transport = { kind = "command", command = "sleep", args = {}, timeout = 5 },
               })"#,
        )
        .expect("it parses");
        assert!(matches!(
            parsed,
            Transport::Command {
                timeout: Some(5),
                ..
            }
        ));
    }

    #[test]
    fn the_three_transports_are_told_apart_by_kind() {
        // One registry, three ways in, and the turn loop cannot tell them apart afterwards.
        assert!(matches!(
            transport(r#"axon.tool("a", { transport = { kind = "lua" } })"#),
            Ok(Transport::Lua)
        ));
        assert!(matches!(
            transport(r#"axon.tool("a", { transport = { kind = "process", command = "x" } })"#),
            Ok(Transport::Process { .. })
        ));
        assert!(matches!(
            transport(r#"axon.tool("a", { transport = { kind = "command", command = "x" } })"#),
            Ok(Transport::Command { .. })
        ));
    }

    #[test]
    fn a_placeholder_the_schema_does_not_declare_is_refused() {
        let tool = axon_tools::command::CommandTool::new(
            "grep",
            "",
            serde_json::json!({
                "type": "object",
                "properties": { "pattern": { "type": "string" } }
            }),
            "rg",
            vec!["{pattern}".to_owned(), "{limit}".to_owned()],
        );
        assert_eq!(
            undeclared(&tool, &tool.parameters()),
            Some("limit".to_owned())
        );
    }

    #[test]
    fn a_declaration_whose_placeholders_all_exist_is_accepted() {
        let tool = axon_tools::command::CommandTool::new(
            "grep",
            "",
            serde_json::json!({
                "type": "object",
                "properties": { "pattern": { "type": "string" } }
            }),
            "rg",
            vec!["{pattern}".to_owned()],
        );
        assert_eq!(undeclared(&tool, &tool.parameters()), None);
    }
}

/// A Lua function told when a tool finishes.
///
/// The seam a memory layer needs and axon's Rust should not know about. axon reports *that a
/// tool ran and whether it worked*; what to do with that — report it to memo, count it, ignore
/// it — is a configuration's business, and lives in Lua beside the client it would use.
///
/// Failures are swallowed on purpose. A watcher that raised would turn observing a tool call
/// into a way of breaking one, and the whole point of watching after the fact is that it cannot.
pub struct LuaWatch {
    engine: Rc<RefCell<Engine>>,
}

impl LuaWatch {
    /// Watch through `engine`.
    #[must_use]
    pub fn new(engine: Rc<RefCell<Engine>>) -> Self {
        Self { engine }
    }
}

impl axon_tools::Watch for LuaWatch {
    fn finished(&self, name: &str, arguments: &serde_json::Value, is_error: bool) {
        let event = serde_json::json!({
            "tool": name,
            "arguments": arguments,
            "is_error": is_error,
        });
        // Borrowed rather than held: a tool's own body may still be on the stack above this,
        // and a watcher that panicked on a double borrow would take the turn with it.
        if let Ok(mut engine) = self.engine.try_borrow_mut() {
            engine.call_watchers(&event);
        }
    }
}
