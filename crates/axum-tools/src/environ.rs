//! The environment every process axum starts is given.
//!
//! One place, because "what does a tool inherit" is a question with one answer and several
//! spawn sites. A peer, the shell peer, and the user's own shell inside it are the same chain:
//! the peer inherits from the daemon, and the shell inherits from the peer, so setting this
//! where a peer is started reaches all of them.

use std::collections::BTreeMap;
use std::process::Command;

/// What is set whether or not anybody asked.
///
/// `OSLO_PROFILE` tells the shell which profile to load. Axum's shell is not an interactive one
/// and should not read the profile a person wrote for theirs — a prompt, a pager, a greeting are
/// noise in a tool's output at best and a hang at worst. Mandatory rather than a default a config
/// could forget: a session that silently sourced the wrong profile is one where every command
/// returns something slightly wrong.
pub const ALWAYS: &[(&str, &str)] = &[("OSLO_PROFILE", "axum")];

/// Apply axum's environment to `command`.
///
/// `extra` is whatever the configuration added, and it is applied second so a config can override
/// one of the mandatory pairs deliberately — a person who says `OSLO_PROFILE = "mine"` has said
/// something, and refusing it would only send them to a wrapper script.
pub fn apply(command: &mut Command, extra: &BTreeMap<String, String>) {
    for (name, value) in ALWAYS {
        command.env(name, value);
    }
    for (name, value) in extra {
        command.env(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a command would run with, read back out of it.
    fn envs(command: &Command) -> BTreeMap<String, String> {
        command
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    #[test]
    fn a_child_gets_the_profile_without_being_asked() {
        let mut command = Command::new("true");
        apply(&mut command, &BTreeMap::new());
        assert_eq!(
            envs(&command).get("OSLO_PROFILE").map(String::as_str),
            Some("axum")
        );
    }

    #[test]
    fn a_configured_pair_is_added_beside_it() {
        let mut command = Command::new("true");
        let extra = BTreeMap::from([("FOO".to_owned(), "bar".to_owned())]);
        apply(&mut command, &extra);
        let seen = envs(&command);
        assert_eq!(seen.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(seen.get("OSLO_PROFILE").map(String::as_str), Some("axum"));
    }

    #[test]
    fn a_configuration_may_override_a_mandatory_pair() {
        // Deliberate, not a hole: somebody who names a profile has said something, and refusing
        // it only moves the decision into a wrapper script where nothing can see it.
        let mut command = Command::new("true");
        let extra = BTreeMap::from([("OSLO_PROFILE".to_owned(), "mine".to_owned())]);
        apply(&mut command, &extra);
        assert_eq!(
            envs(&command).get("OSLO_PROFILE").map(String::as_str),
            Some("mine")
        );
    }

    #[test]
    fn nothing_else_is_cleared() {
        // The child inherits the daemon's environment; this adds to it rather than replacing it,
        // so `PATH`, `HOME` and a person's own exports survive.
        let mut command = Command::new("true");
        apply(&mut command, &BTreeMap::new());
        assert_eq!(envs(&command).len(), ALWAYS.len(), "only what we set");
    }
}
