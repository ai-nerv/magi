//! Active tool visibility and execution filter, driven by the active profile or agent policy.

use std::collections::BTreeSet;

/// Which tools the registry exposes to the model and permits to execute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolFilter {
    /// Every registered tool is available.
    #[default]
    All,
    /// Only tools whose names are listed here are available.
    Allow(BTreeSet<String>),
    /// Every registered tool except those listed here is available.
    Deny(BTreeSet<String>),
}

impl ToolFilter {
    #[must_use]
    pub fn allows(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Allow(names) => names.contains(name),
            Self::Deny(names) => !names.contains(name),
        }
    }

    /// Return an error [`crate::Output`] if `name` is blocked by this filter.
    #[must_use]
    pub fn check(&self, name: &str) -> Option<crate::Output> {
        (!self.allows(name)).then(|| {
            crate::Output::error(format!(
                "tool {name:?} is not available in the active profile"
            ))
        })
    }

    /// Return a narrower filter that also blocks `name`.
    #[must_use]
    pub fn without(&self, name: &str) -> Self {
        match self {
            Self::All => Self::Deny(BTreeSet::from([name.to_owned()])),
            Self::Allow(names) => {
                let mut kept = names.clone();
                kept.remove(name);
                Self::Allow(kept)
            }
            Self::Deny(names) => {
                let mut denied = names.clone();
                denied.insert(name.to_owned());
                Self::Deny(denied)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cancel, Ops, Output, Registry, Tool, Uncancelled, ops::Real};

    struct Dummy(&'static str);

    impl Tool for Dummy {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "dummy"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn run(&self, _: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
            Output::ok("ran")
        }
    }

    #[test]
    fn allow_and_deny_filters_gate_declarations_and_execution() {
        let mut reg = Registry::new();
        reg.register(Box::new(Dummy("read")));
        reg.register(Box::new(Dummy("write")));
        reg.register(Box::new(Dummy("spawn")));
        let ops = Real::new(std::env::temp_dir());

        reg.set_filter(ToolFilter::Deny(BTreeSet::from(["write".to_owned()])).without("spawn"));
        let names: Vec<String> = reg.declarations().into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["read"]);
        assert!(
            !reg.call("read", &serde_json::json!({}), &ops, &Uncancelled)
                .is_error
        );
        let blocked = reg.call("write", &serde_json::json!({}), &ops, &Uncancelled);
        assert!(blocked.is_error);
        assert!(blocked.content.contains("active profile"));
        let prepared = reg.answer("spawn", "{}", &ops, &Uncancelled);
        assert!(prepared.is_error);
        assert!(prepared.content.contains("active profile"));

        reg.set_filter(ToolFilter::Allow(BTreeSet::from(["read".to_owned()])));
        let names: Vec<String> = reg.declarations().into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["read"]);
        assert!(
            reg.call("write", &serde_json::json!({}), &ops, &Uncancelled)
                .is_error
        );
    }
}
