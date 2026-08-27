//! The one registry every tool lands in.

use crate::{Ops, Output};
use std::collections::BTreeMap;

/// Something the model can call.
///
/// Implemented once per transport, never once per tool: `builtin` here, `lua` in `axum-lua`,
/// `process` over the wire. A caller holding a `&dyn Tool` cannot tell which it has.
pub trait Tool: Send + Sync {
    /// The name the model calls it by.
    fn name(&self) -> &str;

    /// What it does, in the model's terms.
    fn description(&self) -> &str;

    /// JSON Schema for its arguments.
    fn parameters(&self) -> serde_json::Value;

    /// Run it.
    ///
    /// Infallible on purpose: every failure a tool can have is something the model should read,
    /// so it comes back as [`Output::error`] rather than as an error the turn loop has to
    /// invent a message for.
    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops) -> Output;
}

/// Every tool the session can reach.
///
/// Keyed by name, so a later declaration replaces an earlier one and the name is the identity.
/// Tau needs per-instance prefixes because two instances of one extension both declare
/// `shell`; a keyed registrar makes that impossible to express in the first place.
#[derive(Default)]
pub struct Registry {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool, replacing any of the same name.
    ///
    /// Replacement rather than refusal because a config that declares `bash` means it: the
    /// point of shipping a default is that it can be overridden.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }

    /// One tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(AsRef::as_ref)
    }

    /// How many tools are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Every tool, as a provider needs them declared.
    #[must_use]
    pub fn declarations(&self) -> Vec<axum_model::Tool> {
        self.tools
            .values()
            .map(|tool| axum_model::Tool {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters: tool.parameters(),
            })
            .collect()
    }

    /// Run a call, or say why it could not be run.
    ///
    /// An unknown tool is an [`Output::error`] rather than a hole: the model asked for
    /// something that does not exist and needs to be told, not left waiting.
    #[must_use]
    pub fn call(&self, name: &str, arguments: &serde_json::Value, ops: &dyn Ops) -> Output {
        match self.get(name) {
            Some(tool) => tool.run(arguments, ops),
            None => {
                let known: Vec<&str> = self.tools.keys().map(String::as_str).collect();
                Output::error(format!(
                    "there is no tool called {name:?}. Available: {}",
                    known.join(", ")
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Real;

    struct Fake(&'static str);

    impl Tool for Fake {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "a fake"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn run(&self, _arguments: &serde_json::Value, _ops: &dyn Ops) -> Output {
            Output::ok(self.0)
        }
    }

    fn ops() -> Real {
        Real::new(std::env::temp_dir())
    }

    #[test]
    fn a_registered_tool_can_be_called() {
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("read")));
        let output = registry.call("read", &serde_json::json!({}), &ops());
        assert_eq!(output.content, "read");
        assert!(!output.is_error);
    }

    #[test]
    fn a_later_declaration_replaces_an_earlier_one() {
        // The point of shipping a default is that a config can override it.
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("bash")));
        registry.register(Box::new(Fake("bash")));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn an_unknown_tool_is_told_so_and_shown_what_exists() {
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("read")));
        let output = registry.call("nope", &serde_json::json!({}), &ops());
        assert!(output.is_error);
        assert!(output.content.contains("nope"), "{}", output.content);
        assert!(output.content.contains("read"), "{}", output.content);
    }

    #[test]
    fn declarations_carry_what_a_provider_needs() {
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("read")));
        let declared = registry.declarations();
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].name, "read");
        assert_eq!(declared[0].parameters["type"], "object");
    }

    #[test]
    fn an_empty_registry_says_so() {
        assert!(Registry::new().is_empty());
    }
}
