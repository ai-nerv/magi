//! Pre/post tool hooks: inspect, rewrite, or block tool arguments before execution and filter or
//! redact tool output before it reaches the model or transcript.

use crate::Output;
use std::cell::RefCell;
use std::rc::Rc;

/// Decision returned by a pre-tool hook.
#[derive(Debug, Clone, PartialEq)]
pub enum Before {
    /// Leave arguments unchanged and continue.
    Continue,
    /// Replace tool arguments before validation, permissions, and execution.
    Rewrite(serde_json::Value),
    /// Refuse the call immediately with a model-readable error message.
    Deny(String),
}

/// Optional edits returned by a post-tool hook.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct After {
    pub content: Option<String>,
    pub is_error: Option<bool>,
}

/// A hook registered around tool execution. Not `Send + Sync`: Lua hooks live in the worker VM.
pub trait Hook {
    fn before(&self, tool: &str, arguments: &serde_json::Value) -> Before {
        let _ = (tool, arguments);
        Before::Continue
    }

    fn after(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        content: &str,
        is_error: bool,
    ) -> Option<After> {
        let _ = (tool, arguments, content, is_error);
        None
    }
}

/// Session hooks shared by a registry. Re-entrant hook calls are skipped rather than panicking.
#[derive(Default, Clone)]
pub struct Hooks(Rc<RefCell<Vec<Box<dyn Hook>>>>);

impl Hooks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, hook: Box<dyn Hook>) {
        self.0.borrow_mut().push(hook);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }

    /// Run all `before` hooks in order. Returns the final arguments, or a refusal reason.
    pub fn run_before(
        &self,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let Ok(hooks) = self.0.try_borrow() else {
            return Ok(arguments);
        };
        let mut current = arguments;
        for hook in hooks.iter() {
            match hook.before(tool, &current) {
                Before::Continue => {}
                Before::Rewrite(next) => current = next,
                Before::Deny(reason) => return Err(reason),
            }
        }
        Ok(current)
    }

    /// Run all `after` hooks in order, allowing each to filter or redact `output.content`.
    pub fn run_after(&self, tool: &str, arguments: &serde_json::Value, output: &mut Output) {
        let Ok(hooks) = self.0.try_borrow() else {
            return;
        };
        for hook in hooks.iter() {
            if let Some(after) = hook.after(tool, arguments, &output.content, output.is_error) {
                if let Some(content) = after.content {
                    output.content = content;
                }
                if let Some(is_error) = after.is_error {
                    output.is_error = is_error;
                }
            }
        }
    }
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.try_borrow() {
            Ok(hooks) => write!(f, "Hooks({})", hooks.len()),
            Err(_) => f.write_str("Hooks(busy)"),
        }
    }
}
