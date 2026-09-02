//! The environment every process axon starts is given.
//!
//! One place, because "what does a tool inherit" is a question with one answer and several
//! spawn sites. A peer, the shell peer, and the user's own shell inside it are the same chain:
//! the peer inherits from the daemon, and the shell inherits from the peer, so setting this
//! where a peer is started reaches all of them.

use std::collections::BTreeMap;
use std::process::Command;

/// What is set whether or not anybody asked.
///
/// `OSLO_PROFILE` names the **history and tracking store** a shell records into — nothing more.
/// Setting it keeps what an agent ran out of the history a person scrolls back through, and keeps
/// it *somewhere*, which is worth having: `oslo history` under this name is the log of what the
/// agent did.
///
/// It was documented here as choosing which profile to *load*, and used as though setting it made
/// the shell non-interactive. It does not, and believing it did hid a real problem for a while —
/// the shell came up with the person's full prompt, redrawing on every keystroke of a command
/// written into it. What a shell should not do when nothing is watching is said by `TERM=dumb`,
/// which the shell peer sets on the terminal it opens.
///
/// Mandatory rather than a default a config could forget: an agent's commands landing in a
/// person's own history is a thing they would have to notice before they could object to it.
pub const ALWAYS: &[(&str, &str)] = &[("OSLO_PROFILE", "axon")];

/// Apply axon's environment to `command`.
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
            Some("axon")
        );
    }

    #[test]
    fn a_configured_pair_is_added_beside_it() {
        let mut command = Command::new("true");
        let extra = BTreeMap::from([("FOO".to_owned(), "bar".to_owned())]);
        apply(&mut command, &extra);
        let seen = envs(&command);
        assert_eq!(seen.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(seen.get("OSLO_PROFILE").map(String::as_str), Some("axon"));
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
