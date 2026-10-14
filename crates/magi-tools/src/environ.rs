//! The environment every process magi starts is given. One place: a peer inherits from the daemon
//! and the shell inherits from the peer, so setting this where a peer starts reaches all of them.

use std::collections::BTreeMap;
use std::process::Command;

/// What is set whether or not anybody asked. `OSLO_PROFILE` names the history and tracking store a
/// shell records into and nothing more — it does not make the shell non-interactive, which is what
/// `TERM=dumb` says. It keeps what an agent ran out of the history a person scrolls back through.
pub const ALWAYS: &[(&str, &str)] = &[("OSLO_PROFILE", "magi")];

/// The variables FAMILY.md lists as carrying capability, which a program that is not one of the
/// family never receives. melchior's attribution, `MAGI_MELCHIOR_PROJECT` and the rest, is not
/// here: `melchior tool` reads it to say which session is calling.
pub const CAPABILITY: &[&str] = &[
    "CASPER_JAIL",
    "CASPER_CONFIGURE",
    "MAGI_TOOLS_CONFIGURE",
    "MAGI_GRANTS",
    "MAGI_NO_SPAWN",
    "MAGI_MEMORY_AGENT",
    "BALTHASAR_AGENT",
    "NERV_GIVEN",
    "MAGI_API_SOCKET",
    "MAGI_HOST_SOCKET",
    "MAGI_PARENT_HOST_SOCKET",
    "MAGI_SESSION_PID",
    "MAGI_COORD_PROGRAM",
    "MAGI_TOOLS_DOOR",
    "MAGI_MEMORY_DOOR",
    "MAGI_AGENT_DOOR",
    "NERV_LOG",
    "MELCHIOR_CONFIG",
    "MAGI_MELCHIOR_TOKEN",
    "MAGI_MELCHIOR_TOKEN_FILE",
];

/// Whether `name` carries capability: one of [`CAPABILITY`], or a `*_DEBUG_LOG`.
#[must_use]
pub fn carries_capability(name: &str) -> bool {
    CAPABILITY.contains(&name) || name.ends_with("_DEBUG_LOG")
}

/// Take every capability variable this process holds out of what `command` inherits.
pub fn without_capability(command: &mut Command) {
    for name in CAPABILITY {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(carries_capability) {
            command.env_remove(name);
        }
    }
}

/// Apply magi's environment to `command`. `extra` is whatever the configuration added, applied
/// second so a config can override one of the mandatory pairs deliberately.
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
            Some("magi")
        );
    }

    #[test]
    fn a_configured_pair_is_added_beside_it() {
        let mut command = Command::new("true");
        let extra = BTreeMap::from([("FOO".to_owned(), "bar".to_owned())]);
        apply(&mut command, &extra);
        let seen = envs(&command);
        assert_eq!(seen.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(seen.get("OSLO_PROFILE").map(String::as_str), Some("magi"));
    }

    #[test]
    fn a_configuration_may_override_a_mandatory_pair() {
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
        // The child inherits the daemon's environment; this adds to it rather than replacing it, so
        // `PATH`, `HOME` and a person's own exports survive.
        let mut command = Command::new("true");
        apply(&mut command, &BTreeMap::new());
        assert_eq!(envs(&command).len(), ALWAYS.len(), "only what we set");
    }
}
