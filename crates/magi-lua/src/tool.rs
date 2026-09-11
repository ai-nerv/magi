//! Tools declared in a config file.
//!
//! `magi.tool(name, spec)` registers one. The spec says what it does, what arguments it takes,
//! and — the part that matters — **how it is reached**:
//!
//! ```lua
//! magi.tool("hexe", {
//!   description = "…", parameters = { … },
//!   transport = { kind = "lua" },
//!   run = function(args, ops) … end,
//! })
//!
//! magi.tool("bash", {
//!   description = "…", parameters = { … },
//!   transport = { kind = "process", command = "magi", args = { "ext", "shell" } },
//! })
//! ```
//!
//! Transport is a property of a declaration rather than a second registry, so adding a way to
//! reach a tool never adds a way to run one: everything lands in [`magi_tools::Registry`] and
//! the turn loop cannot tell them apart.

use crate::Engine;
use magi_tools::{Cancel, Ops, Output, Tool};
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

/// How a declared tool is reached.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Transport {
    /// A Lua function in the worker's VM. No process, no serialisation: it gets [`Ops`] and the
    /// VM's own natives. Deliberately not a shell; a tool that runs commands is a process.
    Lua,
    /// A peer in its own process, spoken to over the wire. Any language, crash-isolated, and what
    /// a tool that must outlive one call or be untrusted should be.
    Process {
        command: String,
        #[serde(default, deserialize_with = "lua_list")]
        args: Vec<String>,
        /// Environment for this peer, beside what every process magi starts already gets.
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
    },
    /// An MCP server: a program that publishes several tools, spoken to in JSON-RPC. The one
    /// declaration that registers more than one tool, so the name a config gives it is the
    /// server's and the names the model sees are the server's own.
    Mcp {
        command: String,
        #[serde(default, deserialize_with = "lua_list")]
        args: Vec<String>,
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
        /// The SHA-256 this server's program must hash to, if it is pinned. An MCP server is
        /// somebody else's code running as you, and `command` resolves to whatever is on `$PATH`
        /// today; a mismatch refuses to start and says both hashes. `magi doctor` prints what each
        /// server actually hashed to.
        #[serde(default)]
        sha256: Option<String>,
    },
    /// An ordinary program magi runs, with arguments built from the call. Not a peer: magi reads
    /// what the child printed. There is no shell — see [`magi_tools::command::render`] for what an
    /// argument is and is not.
    Command {
        command: String,
        /// Its arguments, each a literal or `{name}` naming a declared property.
        #[serde(default, deserialize_with = "lua_list")]
        args: Vec<String>,
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
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_object")]
    pub parameters: serde_json::Value,
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

    // A Lua body runs to completion inside the VM, so there is no point between entering it and
    // leaving it at which an interrupt could be noticed.
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
                // A tool declared here may paint its output too, in the vocabulary casper's use.
                // Absent, or in a shape this build cannot read, is plain text.
                shown: value
                    .get("shown")
                    .and_then(|shown| serde_json::from_value(shown.clone()).ok()),
            },
            // A description that raised, returned nothing, or has no `run` at all. Reported as
            // a result rather than a fault: the model asked for it and needs to be told.
            None => Output::error(format!("the tool {:?} did not answer", self.name)),
        }
    }
}

/// Build the whole registry, in the one order a session uses.
///
/// Two differences between a listing and a session are principled and are parameters here: a
/// listing must not stop to ask a permission question, and it has no screen to lend a tool.
/// Answers the registry and the names the `tools` role's program supplied. Probing is the caller's.
pub fn assemble(
    engine: Rc<RefCell<Engine>>,
    asker: std::sync::Arc<dyn magi_tools::question::Asks>,
    holder: std::sync::Arc<dyn magi_tools::holding::Holds>,
    environ: &std::collections::BTreeMap<String, String>,
    tooling: &magi_tools::supplier::Tooling,
) -> (magi_tools::Registry, std::collections::BTreeSet<String>) {
    let mut registry = magi_tools::Registry::new();

    // The role's program is where the tools come from — magi has none of its own. Registration is
    // keyed and a person's own `tools.lua` is nearest, so a declared name beats the supplied one.
    // Nothing when the program is not installed, so a session then has no tools at all, which
    // `ROLES.md` says is legal.
    let mut supplied = std::collections::BTreeSet::new();
    for tool in magi_tools::supplier::SuppliedTool::pinned(tooling, asker, holder) {
        supplied.insert(tool.name().to_owned());
        registry.register(Box::new(tool));
    }
    // The one builtin that reaches the harness — not a tool in casper's sense but magi coordinating
    // its own agent tree — given the environment it starts a child with.
    magi_tools::builtin::install_spawn(&mut registry, environ);

    // A name a config declared for itself is that config's, however far it also travelled.
    let declared: Vec<String> = engine
        .borrow_mut()
        .tools()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    supplied.retain(|name| !declared.contains(name));

    install(engine, &mut registry, environ);
    (registry, supplied)
}

/// Build every declared tool into one registry, on top of the floor. Both transports land here and
/// the registry cannot tell them apart. A declaration that will not parse is skipped with a reason
/// on stderr: one broken tool should cost you that tool, not the session.
pub fn install(
    engine: Rc<RefCell<Engine>>,
    registry: &mut magi_tools::Registry,
    environ: &std::collections::BTreeMap<String, String>,
) {
    // Installed once, whether or not anything is watching: wiring it conditionally would mean a
    // config that adds a watcher after startup silently never fires.
    registry.watch(Box::new(LuaWatch::new(Rc::clone(&engine))));

    let declared = engine.borrow_mut().tools();
    for (name, spec) in declared {
        let declaration: Declaration = match serde_json::from_value(spec) {
            Ok(declaration) => declaration,
            Err(why) => {
                eprintln!("magi: the tool {name:?} was not registered: {why}");
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
                let tool = magi_tools::command::CommandTool::new(
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
                // Both halves of one rule: the schema and the argument vector name the same things.
                // Either way round, the argument silently vanishes at every call.
                if let Some(unknown) = undeclared(&tool, &declaration.parameters) {
                    eprintln!(
                        "magi: the tool {name:?} was not registered: its arguments name {unknown:?}, which it does not declare"
                    );
                    continue;
                }
                if let Some(dropped) = uncarried(&tool, &declaration.parameters) {
                    eprintln!(
                        "magi: the tool {name:?} was not registered: it declares {dropped:?}, which none of its arguments carry"
                    );
                    continue;
                }
                registry.register(Box::new(tool));
            }
            Transport::Process { command, args, env } => {
                registry.register(Box::new(
                    magi_tools::process::ProcessTool::new(
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
            Transport::Mcp {
                command,
                args,
                env,
                sha256,
            } => {
                // The server is asked what it offers, at load, because that is the only thing that
                // knows. The name this declaration was given is the server's, and is not a tool.
                let environ: std::collections::BTreeMap<String, String> = environ
                    .iter()
                    .chain(env)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                match magi_tools::mcp::McpTool::all(command, args, &environ, sha256.as_deref()) {
                    Ok(tools) => {
                        for tool in tools {
                            registry.register(Box::new(tool));
                        }
                    }
                    // Reported and skipped, like every other tool that will not register.
                    Err(why) => {
                        eprintln!("magi: the MCP server {name:?} offered nothing: {why}");
                    }
                }
            }
        }
    }
}

/// A list that may arrive as an empty table. Lua has one table type, so `{}` is both an empty
/// array and an empty object; reading it as an object would make `args = {}` — the ordinary way to
/// say "no arguments" — a type error.
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

/// An MCP server, declared in a config and reached through the registry. Split under THE RULE.
#[cfg(test)]
#[path = "tool/mcp.rs"]
mod mcp_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use magi_tools::Registry;
    use magi_tools::ops::Real;

    pub(super) fn built(source: &str) -> (Registry, Rc<RefCell<Engine>>) {
        let mut engine = Engine::new();
        engine
            .run(source, "tools.lua")
            .expect("the config must run");
        let engine = Rc::new(RefCell::new(engine));
        let mut registry = Registry::new();
        install(Rc::clone(&engine), &mut registry, &Default::default());
        (registry, engine)
    }

    fn ops() -> Real {
        Real::new(std::env::temp_dir())
    }

    const LUA_TOOL: &str = r#"
        magi.tool("echo", {
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
            &magi_tools::Uncancelled,
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
                    &magi_tools::Uncancelled
                )
                .is_error
        );
    }

    #[test]
    fn a_lua_tool_that_raises_fails_the_call_not_the_turn() {
        let (registry, _) = built(
            r#"
            magi.tool("boom", {
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
            &magi_tools::Uncancelled,
        );
        assert!(output.is_error);
        assert!(output.content.contains("deliberate"), "{}", output.content);
    }

    #[test]
    fn a_tool_may_report_a_failure_the_model_should_read() {
        let (registry, _) = built(
            r#"
            magi.tool("nope", {
              transport = { kind = "lua" },
              run = function() return { content = "no such thing", is_error = true } end,
            })
            "#,
        );
        let output = registry.call(
            "nope",
            &serde_json::json!({}),
            &ops(),
            &magi_tools::Uncancelled,
        );
        assert!(output.is_error);
        assert_eq!(output.content, "no such thing");
    }

    #[test]
    fn both_transports_land_in_one_registry() {
        let (registry, _) = built(
            r#"
            magi.tool("a-lua", { transport = { kind = "lua" }, run = function() return "x" end })
            magi.tool("a-process", {
              transport = { kind = "process", command = "true", args = {} },
            })
            "#,
        );
        // Both declarations, and nothing distinguishes them from outside. magi registers no tools
        // of its own — the floor is casper's, absent from this bare `built` registry.
        assert_eq!(registry.len(), 2);
        for name in ["a-lua", "a-process"] {
            assert!(registry.get(name).is_some(), "{name} is missing");
        }
    }

    #[test]
    fn a_declaration_registers_under_its_name() {
        // A config can declare any name — including one casper also supplies, which a keyed
        // registration then replaces (that override is exercised in `assemble`, with a supplier).
        let (registry, _) = built(
            r#"
            magi.tool("read", {
              description = "Mine instead.",
              transport = { kind = "lua" },
              run = function() return "mine" end,
            })
            "#,
        );
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry
                .call(
                    "read",
                    &serde_json::json!({}),
                    &ops(),
                    &magi_tools::Uncancelled
                )
                .content,
            "mine"
        );
    }

    #[test]
    fn a_malformed_declaration_costs_only_that_tool() {
        let (registry, _) = built(
            r#"
            magi.tool("broken", { transport = { kind = "carrier-pigeon" } })
            magi.tool("fine", { transport = { kind = "lua" }, run = function() return "ok" end })
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

/// A placeholder in a command's arguments that its schema never declares. `{limit}` against a
/// schema with no `limit` property can never be filled, so the argument disappears from every call
/// and the tool quietly runs unbounded.
fn undeclared(
    tool: &magi_tools::command::CommandTool,
    parameters: &serde_json::Value,
) -> Option<String> {
    let declared = parameters.get("properties").and_then(|p| p.as_object());
    tool.placeholders()
        .into_iter()
        .find(|name| !declared.is_some_and(|properties| properties.contains_key(name)))
}

/// A property the schema declares that no argument carries. The model is told it may send `role`,
/// spends a call sending it, and the value never reaches the program — which then refuses for want
/// of the thing that was in fact supplied.
fn uncarried(
    tool: &magi_tools::command::CommandTool,
    parameters: &serde_json::Value,
) -> Option<String> {
    let carried = tool.placeholders();
    parameters
        .get("properties")
        .and_then(|properties| properties.as_object())?
        .keys()
        .find(|name| !carried.contains(name))
        .cloned()
}

#[cfg(test)]
mod command_transport {
    use super::tests::built;
    use super::*;

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
            r#"magi.tool("say", {
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
            r#"magi.tool("slow", {
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
            transport(r#"magi.tool("a", { transport = { kind = "lua" } })"#),
            Ok(Transport::Lua)
        ));
        assert!(matches!(
            transport(r#"magi.tool("a", { transport = { kind = "process", command = "x" } })"#),
            Ok(Transport::Process { .. })
        ));
        assert!(matches!(
            transport(r#"magi.tool("a", { transport = { kind = "command", command = "x" } })"#),
            Ok(Transport::Command { .. })
        ));
    }

    #[test]
    fn a_placeholder_the_schema_does_not_declare_is_refused() {
        let tool = magi_tools::command::CommandTool::new(
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
        let tool = magi_tools::command::CommandTool::new(
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
        assert_eq!(uncarried(&tool, &tool.parameters()), None);
    }

    #[test]
    fn a_property_no_argument_carries_is_refused() {
        // The other half of the same rule. The model is offered `limit`, fills it, and `rg` is run
        // without it -- the schema promising something the argument vector cannot deliver.
        let tool = magi_tools::command::CommandTool::new(
            "grep",
            "",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "limit": { "type": "integer" }
                }
            }),
            "rg",
            vec!["{pattern}".to_owned()],
        );
        assert_eq!(
            uncarried(&tool, &tool.parameters()),
            Some("limit".to_owned())
        );
    }

    #[test]
    fn a_command_promising_an_argument_it_drops_does_not_register() {
        let (registry, _) = built(
            r#"
            magi.tool("half", {
              description = "takes two and passes one",
              parameters = {
                type = "object",
                properties = { one = { type = "string" }, two = { type = "string" } },
              },
              transport = { kind = "command", command = "echo", args = { "{one}" } },
            })
            "#,
        );
        assert!(registry.get("half").is_none());
    }
}

/// A Lua function told what happened. magi reports what happened; what to do with it is a
/// configuration's business. Failures are swallowed on purpose: a watcher that raised would turn
/// observing a session into a way of breaking one.
pub struct LuaWatch {
    engine: Rc<RefCell<Engine>>,
}

impl LuaWatch {
    #[must_use]
    pub fn new(engine: Rc<RefCell<Engine>>) -> Self {
        Self { engine }
    }
}

impl magi_tools::Watch for LuaWatch {
    fn saw(&self, event: &magi_tools::Event<'_>) {
        // Borrowed rather than held: a tool's own body may still be on the stack above this,
        // and a watcher that panicked on a double borrow would take the turn with it.
        if let Ok(mut engine) = self.engine.try_borrow_mut() {
            engine.call_watchers(&event.value());
        }
    }
}
