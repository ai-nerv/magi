//! Naming another axon, and what you are allowed to do to it.
//!
//! `$gamma` and `$main/delta` are both instances and they are not the same kind of thing. One
//! you forked and one you found, and the difference decides what the verbs mean rather than
//! being a note in the documentation:
//!
//! | | | |
//! |---|---|---|
//! | `$gamma` | a **fork** | you started it: ask it, tell it, stop it |
//! | `$main/delta` | a **peer** | somebody else's: ask it, tell it, and that is all |
//!
//! Both can be *spoken to*. A message lands in an inbox and the session it belongs to decides
//! what to do with it, which is not control -- it is the same thing a person does with a message.
//! What separates the two is the one act the far end cannot decline: a peer that could be stopped
//! by anything that learned its name is a session somebody loses while they are typing into it.
//!
//! So [`Kind`] is not decoration: [`Reach::allows`] is the one place that answers "may I", and
//! every verb goes through it.
//!
//! # What is here and what is not
//!
//! Addressing and permission. The supervision — starting a fork, watching it, reaping it — is
//! not written yet, and neither is the routing that turns `tell $gamma to stop` into a message.
//! What exists is the half both of those need first: a name that parses, a socket path it
//! resolves to, and a rule about who may do what.

pub mod answering;
pub mod asking;
pub mod serving;
pub mod wire;

use crate::identity::Identity;
use std::path::{Path, PathBuf};

/// How you came by an instance, which is what decides what you may do to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// You forked it. It is yours: ask it, steer it, stop it.
    Fork,
    /// Somebody else started it. Ask it, and nothing else.
    Peer,
}

/// What a caller is asking to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Read something: who it is, what it is doing, what it has been told.
    Ask,
    /// Put a message in its inbox.
    Tell,
    /// End it.
    Stop,
}

impl Reach {
    /// Whether this may be done to an instance of `kind`.
    ///
    /// The whole of the permission model, in one place, so a verb added later cannot quietly
    /// forget to check. A peer is *queryable* and nothing more: it belongs to whoever started
    /// it, and a session that any process knowing its name can end is a session somebody loses
    /// while they are typing into it.
    #[must_use]
    pub fn allows(self, kind: Kind) -> bool {
        match kind {
            Kind::Fork => true,
            // A peer can be spoken to. Telling something a message is not controlling it: it
            // lands in an inbox and the session it belongs to decides what to do with it, the
            // same way a person reads a message and answers or does not. Ending a session is
            // the one thing the far end does not get to decline, and that is why it is the one
            // thing a peer is refused.
            Kind::Peer => self != Self::Stop,
        }
    }

    /// Why it was refused, for saying so.
    #[must_use]
    pub fn refusal(self, address: &Address) -> String {
        let verb = match self {
            Self::Ask => "ask",
            Self::Tell => "tell",
            Self::Stop => "stop",
        };
        format!(
            "{} is a peer, not a fork: you can ask it and tell it things, but not {verb} it",
            address.written()
        )
    }
}

/// Another instance, by name.
///
/// The full form is `project/role/id`, the same three parts the prompt box wears, and the short
/// forms fill in from whoever is asking: `$delta` is a sibling in this project and role, and
/// `$main/delta` names the role because there will be more than one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Which project, or `None` to mean the asker's own.
    pub project: Option<String>,
    /// Which role, or `None` to mean the asker's own.
    pub role: Option<String>,
    /// Which instance. Always given: this is the part that names one.
    pub id: String,
}

impl Address {
    /// Read `$gamma`, `$main/delta` or `$axon/main/delta`.
    ///
    /// The sigil is optional so this reads what the trigger hands over as well as what somebody
    /// wrote. Answers `None` for anything with no id in it, because an address that names no
    /// instance is not a short form of anything.
    #[must_use]
    pub fn read(written: &str) -> Option<Self> {
        let body = written.strip_prefix('$').unwrap_or(written);
        let parts: Vec<&str> = body.split('/').filter(|part| !part.is_empty()).collect();
        // Read from the right: the last part is always the id, and what comes before it fills
        // in from the outside. `a/b/c/d` is not a deeper address, it is a typo.
        match parts.as_slice() {
            [id] => Some(Self {
                project: None,
                role: None,
                id: (*id).to_owned(),
            }),
            [role, id] => Some(Self {
                project: None,
                role: Some((*role).to_owned()),
                id: (*id).to_owned(),
            }),
            [project, role, id] => Some(Self {
                project: Some((*project).to_owned()),
                role: Some((*role).to_owned()),
                id: (*id).to_owned(),
            }),
            _ => None,
        }
    }

    /// The address as somebody would type it.
    #[must_use]
    pub fn written(&self) -> String {
        let mut out = String::from("$");
        if let Some(project) = &self.project {
            out.push_str(project);
            out.push('/');
        }
        if let Some(role) = &self.role {
            out.push_str(role);
            out.push('/');
        }
        out.push_str(&self.id);
        out
    }

    /// The full three-part name, filling the gaps in from whoever is asking.
    #[must_use]
    pub fn against(&self, asker: &Identity) -> Identity {
        Identity {
            project: self
                .project
                .clone()
                .unwrap_or_else(|| asker.project.clone()),
            role: self.role.clone().unwrap_or_else(|| asker.role.clone()),
            id: self.id.clone(),
        }
    }

    /// Where that instance listens.
    ///
    /// Beside the daemon sockets and named for the instance rather than digested from a path:
    /// a peer is found *by name*, and a name you have to hash a directory to reconstruct is a
    /// name nobody can type. The daemon's own socket stays digested, because that one is found
    /// by the directory you are standing in.
    #[must_use]
    pub fn socket(&self, asker: &Identity) -> PathBuf {
        let whole = self.against(asker);
        runtime()
            .join("axon")
            .join("instances")
            .join(format!("{}.sock", safe(&whole.full())))
    }
}

/// Where a socket for `me` is put, so an instance can be reached by name.
#[must_use]
pub fn listening_at(me: &Identity) -> PathBuf {
    runtime()
        .join("axon")
        .join("instances")
        .join(format!("{}.sock", safe(&me.full())))
}

/// The directory sockets live in.
fn runtime() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Flatten a name into one path segment.
///
/// `project/role/id` has slashes in it and a socket path is a filename. Anything that is not a
/// letter, digit, dash or underscore becomes a dash, so a project called `../etc` cannot name a
/// socket outside the directory this chose.
fn safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Every instance currently listening.
///
/// Read from the directory rather than from a registry somebody has to keep up to date: a
/// process that died did not get to remove itself from a list, and a socket file that nothing
/// answers is discovered on the first call rather than trusted forever.
#[must_use]
pub fn listening() -> Vec<String> {
    let dir = runtime().join("axon").join("instances");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".sock").map(ToOwned::to_owned)
        })
        .collect();
    out.sort();
    out
}

/// Whether a path is one this process may listen on.
///
/// Belt and braces against [`safe`]: a socket path is built from a name that came off a wire,
/// and a name is not a promise.
#[must_use]
pub fn inside(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| parent == runtime().join("axon").join("instances"))
}

/// A name parses, resolves and cannot escape the directory it belongs in.
#[cfg(test)]
mod tests {
    use super::*;

    fn asker() -> Identity {
        Identity {
            project: "axon".to_owned(),
            role: "main".to_owned(),
            id: "alpha".to_owned(),
        }
    }

    #[test]
    fn a_bare_name_is_a_sibling() {
        let address = Address::read("$gamma").expect("an address");
        assert_eq!(address.id, "gamma");
        assert_eq!(address.against(&asker()).full(), "axon/main/gamma");
    }

    #[test]
    fn a_two_part_name_gives_the_role() {
        let address = Address::read("$main/delta").expect("an address");
        assert_eq!(address.role.as_deref(), Some("main"));
        assert_eq!(address.against(&asker()).full(), "axon/main/delta");
    }

    #[test]
    fn a_three_part_name_gives_everything() {
        let address = Address::read("$other/review/eta").expect("an address");
        assert_eq!(address.against(&asker()).full(), "other/review/eta");
    }

    #[test]
    fn the_sigil_is_optional_so_a_trigger_token_reads_the_same() {
        assert_eq!(Address::read("gamma"), Address::read("$gamma"));
    }

    #[test]
    fn an_address_that_names_nobody_is_not_an_address() {
        assert_eq!(Address::read("$"), None);
        assert_eq!(Address::read(""), None);
        assert_eq!(Address::read("$a/b/c/d"), None, "four parts is a typo");
    }

    #[test]
    fn what_was_written_comes_back_out() {
        for written in ["$gamma", "$main/delta", "$other/review/eta"] {
            let address = Address::read(written).expect("an address");
            assert_eq!(address.written(), written);
        }
    }

    #[test]
    fn a_fork_may_be_stopped_and_a_peer_may_not() {
        // The whole permission model, and the line is drawn at the one act the far end cannot
        // decline. A message lands in an inbox and the session decides what to do with it; a
        // session any process knowing its name can end is one somebody loses while typing.
        for reach in [Reach::Ask, Reach::Tell, Reach::Stop] {
            assert!(reach.allows(Kind::Fork), "{reach:?} on your own fork");
        }
        assert!(Reach::Ask.allows(Kind::Peer), "asking is always allowed");
        assert!(
            Reach::Tell.allows(Kind::Peer),
            "a peer you did not fork can still be spoken to"
        );
        assert!(!Reach::Stop.allows(Kind::Peer), "but not ended");
    }

    #[test]
    fn a_refusal_says_which_instance_and_what_was_wanted() {
        let address = Address::read("$main/delta").expect("an address");
        let said = Reach::Stop.refusal(&address);
        assert!(said.contains("$main/delta"), "{said}");
        assert!(said.contains("stop"), "{said}");
    }

    #[test]
    fn a_socket_is_one_segment_inside_the_instances_directory() {
        // `project/role/id` has slashes in it and a socket path is a filename.
        let path = Address::read("$gamma")
            .expect("an address")
            .socket(&asker());
        assert!(inside(&path), "{path:?}");
        let name = path.file_name().expect("a name").to_string_lossy();
        assert!(!name.contains('/'), "{name}");
    }

    #[test]
    fn a_name_cannot_climb_out_of_the_directory() {
        // The reason names are flattened rather than joined. This one comes off a wire.
        let escaping = Address {
            project: Some("../../etc".to_owned()),
            role: Some("..".to_owned()),
            id: "passwd".to_owned(),
        };
        let path = escaping.socket(&asker());
        assert!(inside(&path), "it escaped to {path:?}");
        assert!(
            !path.to_string_lossy().contains(".."),
            "it kept the climb: {path:?}"
        );
    }

    #[test]
    fn listening_answers_nothing_rather_than_failing_with_no_directory() {
        // Nothing has forked yet, which is every session until one does.
        let _ = listening();
    }
}
