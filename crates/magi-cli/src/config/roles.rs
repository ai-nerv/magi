//! Which program fills each role. `ROLES.md` says what a role is *for*; this says who is doing it
//! here, and it is the only place that knows. Three lists used to say it separately — the siblings
//! magi drives, the siblings it borrows a client library from, and the siblings `magi doctor`
//! reports on — and two of them had already drifted apart.

use super::Loaded;

/// One role, and how to find out which program is filling it.
pub struct Role {
    /// `ROLES.md`'s name for it, which is also `scripts/gate-role.sh`'s first argument.
    pub name: &'static str,
    /// The settings that may name its program, in the order they are tried.
    pub named: &'static [&'static str],
    /// Filled by this when no setting names one, so a configuration that says nothing behaves as
    /// it always has.
    pub fallback: &'static str,
    /// The verbs `ROLES.md` calls core: refuse one and the program cannot fill the role. The same
    /// list `scripts/gate-role.sh` holds a candidate to, so `magi doctor` can say a program does
    /// not fill the role it was named for rather than letting the first call of a turn find out.
    pub core: &'static [&'static str],
}

/// The memory layer this build grew up against, and the default for the `memory` role.
pub use magi_host::scribe::BALTHASAR;

/// Every role a session fills. The order is the order `magi doctor` reports them in.
pub const ROLES: &[Role] = &[
    Role {
        name: "memory",
        named: &["memory"],
        fallback: BALTHASAR,
        core: &["observe", "replay", "sessions"],
    },
    Role {
        name: "tools",
        named: &["tools"],
        fallback: magi_tools::supplier::CASPER,
        core: &["tools", "run"],
    },
    // `magi.model` is taken: it names the *model*, not the program that serves models, and has
    // since before roles existed. `magi.melchior` is the name that has always meant this one.
    Role {
        name: "model",
        named: &["melchior"],
        fallback: magi_host::broker::MELCHIOR,
        core: &["models", "ask"],
    },
];

/// The role by that name, or nothing.
#[must_use]
pub fn of(name: &str) -> Option<&'static Role> {
    ROLES.iter().find(|role| role.name == name)
}

/// Which program fills `role` here. An unknown role is nobody's, which is a caller's bug rather
/// than a configuration's.
#[must_use]
pub fn fills(loaded: &Loaded, role: &str) -> String {
    let Some(role) = of(role) else {
        return String::new();
    };
    role.named
        .iter()
        .find_map(|name| loaded.config.string(name))
        .unwrap_or(role.fallback)
        .to_owned()
}

/// Every role and the program filling it here, in the table's order.
#[must_use]
pub fn filled(loaded: &Loaded) -> Vec<(String, String)> {
    ROLES
        .iter()
        .map(|role| (role.name.to_owned(), fills(loaded, role.name)))
        .collect()
}

/// The same, read out of a VM that has run the configuration but has not yet been harvested: the
/// roles decide which siblings are asked for a client library, and that happens before the harvest.
#[must_use]
pub fn said(engine: &mut magi_lua::Engine) -> Vec<(String, String)> {
    ROLES
        .iter()
        .map(|role| {
            let program = role
                .named
                .iter()
                .find_map(|name| engine.setting(name))
                .unwrap_or_else(|| role.fallback.to_owned());
            (role.name.to_owned(), program)
        })
        .collect()
}

/// Every program a session reaches for, one per role and each named once. A program filling two
/// roles is asked for its client library once and driven once.
#[must_use]
pub fn programs(filled: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, program) in filled {
        if !out.iter().any(|held| held == program) {
            out.push(program.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_lua::Engine;

    fn loaded(source: &str) -> Loaded {
        let mut engine = Engine::new();
        engine.run(source, "test").expect("config");
        engine.harvest();
        Loaded {
            config: engine.config(),
            tools: Vec::new(),
            clients: Vec::new(),
        }
    }

    #[test]
    fn a_configuration_that_says_nothing_fills_the_roles_as_it_always_did() {
        let held = loaded("");
        assert_eq!(fills(&held, "memory"), "balthasar");
        assert_eq!(fills(&held, "tools"), "casper");
        assert_eq!(fills(&held, "model"), "melchior");
    }

    #[test]
    fn a_role_is_filled_by_whatever_names_it() {
        let held = loaded(r#"magi.memory = "remembrance""#);
        assert_eq!(fills(&held, "memory"), "remembrance");
        assert_eq!(fills(&held, "tools"), "casper", "and nothing else moved");
    }

    #[test]
    fn the_older_name_for_the_model_still_names_it() {
        // The one role that was swappable before this existed; a configuration using it must not
        // stop working because the concept got a name.
        let held = loaded(r#"magi.melchior = "my-broker""#);
        assert_eq!(fills(&held, "model"), "my-broker");
    }

    #[test]
    fn naming_the_model_does_not_name_a_program() {
        // `magi.model` is the model, and reading it as a role would have magi spawn a model id.
        let held = loaded(r#"magi.model = "openrouter/anthropic/claude-sonnet-4.5""#);
        assert_eq!(fills(&held, "model"), "melchior");
    }

    #[test]
    fn a_siblings_settings_block_is_not_mistaken_for_a_name() {
        // `magi.melchior = { … }` is what that sibling takes, not who it is.
        let held = loaded(r#"magi.melchior = { max_tokens = 4000 }"#);
        assert_eq!(fills(&held, "model"), "melchior");
    }

    #[test]
    fn one_program_filling_two_roles_is_reached_for_once() {
        let filled = vec![
            ("memory".to_owned(), "omni".to_owned()),
            ("tools".to_owned(), "omni".to_owned()),
            ("model".to_owned(), "melchior".to_owned()),
        ];
        assert_eq!(programs(&filled), vec!["omni", "melchior"]);
    }

    #[test]
    fn what_a_vm_says_is_what_the_config_says() {
        let mut engine = Engine::new();
        engine
            .run(r#"magi.memory = "remembrance""#, "test")
            .expect("config");
        let said = said(&mut engine);
        assert_eq!(
            said,
            vec![
                ("memory".to_owned(), "remembrance".to_owned()),
                ("tools".to_owned(), "casper".to_owned()),
                ("model".to_owned(), "melchior".to_owned()),
            ]
        );
    }
}
