//! The kinds of agent the configuration describes — their roles — under `magi.agents`. A role is
//! what a lead chooses a child by, what that child is told, and optionally its model and whether it
//! may start children of its own. `main` is the session nobody started. Not `magi.roles`: that one
//! names which program fills memory, tools and models.
//!
//! ```lua
//! magi.agents = {
//!   reviewer = {
//!     description = "Reviews a change for bugs; changes nothing.",
//!     prompt      = "You are a reviewer. ...",
//!     model       = "openrouter/...",  -- optional
//!     delegate    = false,             -- may it start children of its own; default true
//!   },
//! }
//! ```

use super::Loaded;
use std::collections::BTreeMap;

/// The role a session nobody started is in.
pub const MAIN: &str = "main";

/// One role, as the configuration describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub name: String,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub model: Option<String>,
    pub delegate: bool,
}

/// Every role the configuration describes, by name. An entry that is not a table is skipped.
#[must_use]
pub fn all(loaded: &Loaded) -> Vec<Role> {
    let Some(table) = loaded.config.get("agents").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let text = |held: &serde_json::Map<String, serde_json::Value>, key: &str| {
        held.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|said| !said.is_empty())
            .map(ToOwned::to_owned)
    };
    let mut roles: Vec<Role> = table
        .iter()
        .filter_map(|(name, value)| {
            let held = value.as_object()?;
            Some(Role {
                name: name.clone(),
                description: text(held, "description"),
                prompt: text(held, "prompt"),
                model: text(held, "model"),
                delegate: held
                    .get("delegate")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
            })
        })
        .collect();
    roles.sort_by(|one, two| one.name.cmp(&two.name));
    roles
}

/// The role this session is in: the first line of what melchior wrote in at birth, and `main` for
/// a session nobody started.
#[must_use]
pub fn own_name() -> String {
    std::env::var("MAGI_MELCHIOR_ROLE")
        .ok()
        .and_then(|said| {
            said.lines()
                .next()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| MAIN.to_owned())
}

/// What the configuration says about this session's own role, when it says anything.
#[must_use]
pub fn own(loaded: &Loaded) -> Option<Role> {
    let name = own_name();
    all(loaded).into_iter().find(|role| role.name == name)
}

/// The roles a child can be given, as `spawn` offers them: every one but `main`.
#[must_use]
pub fn kinds(loaded: &Loaded) -> Vec<magi_tools::builtin::Kind> {
    all(loaded)
        .into_iter()
        .filter(|role| role.name != MAIN)
        .map(|role| magi_tools::builtin::Kind {
            name: role.name,
            description: role.description.unwrap_or_default(),
            delegate: role.delegate,
        })
        .collect()
}

/// What each role is for, by name, for the agents view.
#[must_use]
pub fn descriptions(loaded: &Loaded) -> BTreeMap<String, String> {
    all(loaded)
        .into_iter()
        .filter_map(|role| Some((role.name, role.description?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(config: &str) -> Loaded {
        let mut engine = magi_lua::Engine::new();
        engine.run(config, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
        }
    }

    const TWO: &str = r#"magi.agents = {
        reviewer = { description = "reads a change", prompt = "Edit nothing.", delegate = false },
        main = { prompt = "Coordinate." },
    }"#;

    #[test]
    fn a_role_is_what_the_configuration_says_and_the_rest_defaults() {
        let roles = all(&loaded(TWO));
        let reviewer = roles
            .iter()
            .find(|r| r.name == "reviewer")
            .expect("reviewer");
        assert_eq!(reviewer.prompt.as_deref(), Some("Edit nothing."));
        assert!(!reviewer.delegate);
        let main = roles.iter().find(|r| r.name == "main").expect("main");
        assert!(main.delegate, "leave to delegate unless it says otherwise");
        assert_eq!(main.model, None);
    }

    #[test]
    fn main_is_not_a_role_a_child_can_be_given() {
        let offered = kinds(&loaded(TWO));
        let names: Vec<&str> = offered.iter().map(|kind| kind.name.as_str()).collect();
        assert_eq!(names, ["reviewer"]);
    }

    #[test]
    fn only_a_described_role_has_a_description_to_show() {
        let said = descriptions(&loaded(TWO));
        assert_eq!(
            said.get("reviewer").map(String::as_str),
            Some("reads a change")
        );
        assert!(!said.contains_key("main"));
    }

    #[test]
    fn no_table_is_no_roles() {
        assert!(all(&loaded(r#"magi.model = "x""#)).is_empty());
    }
}
