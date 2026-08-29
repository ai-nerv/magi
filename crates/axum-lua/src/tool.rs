//! Tools declared in a config file.
//!
//! `axum.tool(name, spec)` registers one. The spec says what it does, what arguments it takes,
//! and — the part that matters — **how it is reached**:
//!
//! ```lua
//! axum.tool("hexe", {
//!   description = "…", parameters = { … },
//!   transport = { kind = "lua" },
//!   run = function(args, ops) … end,
//! })
//!
//! axum.tool("bash", {
//!   description = "…", parameters = { … },
//!   transport = { kind = "process", command = "axum", args = { "ext", "shell" } },
//! })
//! ```
//!
//! Transport is a property of a declaration rather than a second registry, so adding a way to
//! reach a tool never adds a way to run one: everything lands in [`axum_tools::Registry`] and
//! the turn loop cannot tell them apart.

use crate::Engine;
use axum_tools::{Cancel, Ops, Output, Tool};
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
        /// Environment for this peer, beside what every process axum starts already gets.
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
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
    registry: &mut axum_tools::Registry,
    environ: &std::collections::BTreeMap<String, String>,
) {
    let declared = engine.borrow_mut().tools();
    for (name, spec) in declared {
        let declaration: Declaration = match serde_json::from_value(spec) {
            Ok(declaration) => declaration,
            Err(why) => {
                eprintln!("axum: the tool {name:?} was not registered: {why}");
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
            Transport::Process { command, args, env } => {
                registry.register(Box::new(
                    axum_tools::process::ProcessTool::new(
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
    use axum_tools::Registry;
    use axum_tools::ops::Real;

    /// Run a config chunk and build what it declared.
    fn built(source: &str) -> (Registry, Rc<RefCell<Engine>>) {
        let mut engine = Engine::new();
        engine
            .run(source, "tools.lua")
            .expect("the config must run");
        let engine = Rc::new(RefCell::new(engine));
        let mut registry = Registry::new();
        axum_tools::builtin::install(&mut registry);
        install(Rc::clone(&engine), &mut registry, &Default::default());
        (registry, engine)
    }

    fn ops() -> Real {
        Real::new(std::env::temp_dir())
    }

    const LUA_TOOL: &str = r#"
        axum.tool("echo", {
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
            &axum_tools::Uncancelled,
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
                    &axum_tools::Uncancelled
                )
                .is_error
        );
    }

    #[test]
    fn a_lua_tool_that_raises_fails_the_call_not_the_turn() {
        let (registry, _) = built(
            r#"
            axum.tool("boom", {
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
            &axum_tools::Uncancelled,
        );
        assert!(output.is_error);
        assert!(output.content.contains("deliberate"), "{}", output.content);
    }

    #[test]
    fn a_tool_may_report_a_failure_the_model_should_read() {
        let (registry, _) = built(
            r#"
            axum.tool("nope", {
              transport = { kind = "lua" },
              run = function() return { content = "no such thing", is_error = true } end,
            })
            "#,
        );
        let output = registry.call(
            "nope",
            &serde_json::json!({}),
            &ops(),
            &axum_tools::Uncancelled,
        );
        assert!(output.is_error);
        assert_eq!(output.content, "no such thing");
    }

    #[test]
    fn both_transports_land_in_one_registry() {
        let (registry, _) = built(
            r#"
            axum.tool("a-lua", { transport = { kind = "lua" }, run = function() return "x" end })
            axum.tool("a-process", {
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
            axum.tool("read", {
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
                    &axum_tools::Uncancelled
                )
                .content,
            "mine"
        );
    }

    #[test]
    fn a_malformed_declaration_costs_only_that_tool() {
        let (registry, _) = built(
            r#"
            axum.tool("broken", { transport = { kind = "carrier-pigeon" } })
            axum.tool("fine", { transport = { kind = "lua" }, run = function() return "ok" end })
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
