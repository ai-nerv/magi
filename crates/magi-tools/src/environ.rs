//! The environment every process magi starts is given. One place: a peer inherits from the daemon
//! and the shell inherits from the peer, so setting this where a peer starts reaches all of them.

use std::collections::BTreeMap;
use std::process::Command;

/// What is set whether or not anybody asked. `OSLO_PROFILE` names the history and tracking store a
/// shell records into and nothing more — it does not make the shell non-interactive, which is what
/// `TERM=dumb` says. It keeps what an agent ran out of the history a person scrolls back through.
pub const ALWAYS: &[(&str, &str)] = &[("OSLO_PROFILE", "magi")];

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
