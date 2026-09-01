//! Naming another axon, and where it lives.
//!
//! # The layout
//!
//! ```text
//! $XDG_RUNTIME_DIR/axon/
//!   axon/                  <- one directory per project
//!     alpha-rho            <- a socket, named by the id and nothing else
//!     iota-mu
//!     iota-mu.parent       <- "alpha-rho": who started it
//!   other-project/
//!     beta-nu
//! ```
//!
//! A project directory, and inside it a socket per session named by its id. No role in the
//! path, because a role is not part of a name — see [`crate::identity`]. Sessions in different
//! projects are not refused each other, they are *not in each other's directory*, which is the
//! project wall from [`policy`] enforced by the filesystem rather than by a check somebody could
//! forget to write.
//!
//! The `.parent` file beside a subagent's socket says who started it. That is what makes the
//! tree legible: a session that finds `iota-mu` in the directory can tell it is behind
//! `alpha-rho`'s door without asking it, and without trusting what it would have said.
//!
//! # What a caller's name is worth
//!
//! Every call says who is making it, and for `ask` and `tell` that claim is taken at face
//! value. It has to be: everything here runs as one user in one directory, so any process that
//! can open the socket could open it claiming anything, and a check that cannot be enforced is
//! worse than none — it reads like security to whoever comes along next.
//!
//! `stop` is the exception, because it is the one act the far end cannot decline. It carries the
//! secret handed to the session in [`TOKEN`] when it was started, which only whoever started it
//! ever held. A session nobody started holds none, so nothing can stop it.

pub mod answering;
pub mod briefing;
pub mod policy;
pub mod serving;
pub mod tool;
pub mod wire;

use crate::identity::Identity;
use policy::Whom;
use std::path::{Path, PathBuf};

/// What the model calls the tool that reaches other instances.
///
/// Named once, because it is said in three places: the tool registers under it, the briefing
/// tells the model to use it, and its own help repeats it.
pub const TOOL: &str = "agent";

/// The variable a spawned instance learns its parent from.
///
/// Inherited across the spawn rather than passed as an argument, so a child that re-execs or
/// starts a shell that starts another axon still knows where it came from.
pub const PARENT: &str = "AXON_PARENT";

/// The variable carrying the secret that makes a `stop` honourable.
///
/// Minted by the parent, handed to the child, held by both and by nobody else.
pub const TOKEN: &str = "AXON_TOKEN";

/// Who started this session, if anybody did.
///
/// `None` for one somebody started at a terminal, which is most of them — and a session with no
/// parent is a *main*, which is the whole of what that word means here.
#[must_use]
pub fn parent() -> Option<String> {
    std::env::var(PARENT).ok().filter(|name| !name.is_empty())
}

/// The secret this session was started with, if it was started by another.
#[must_use]
pub fn token() -> Option<String> {
    std::env::var(TOKEN)
        .ok()
        .filter(|secret| !secret.is_empty())
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
    /// The verb, for saying so in a refusal.
    #[must_use]
    pub fn named(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Tell => "tell",
            Self::Stop => "stop",
        }
    }
}

/// Another instance, by name.
///
/// `$iota-mu` is one in this project; `$axon/iota-mu` names the project as well. The second
/// form parses and then loses: [`policy`] refuses anything outside the asker's own project, and
/// the directory it would have to be found in is not one this session lists. It reads rather
/// than being rejected as a typo so the refusal can say *why*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Which project, or `None` to mean the asker's own.
    pub project: Option<String>,
    /// Which instance. Always given: this is the part that names one.
    pub id: String,
}

impl Address {
    /// Read `$iota-mu` or `$axon/iota-mu`.
    ///
    /// The sigil is optional so this reads what the trigger hands over as well as what somebody
    /// wrote. Answers `None` for anything with no id in it, and for three parts — that shape was
    /// `project/role/id` before roles left names, and reading it now would resolve an old name
    /// to a session that is not the one it meant.
    #[must_use]
    pub fn read(written: &str) -> Option<Self> {
        let body = written.strip_prefix('$').unwrap_or(written);
        let parts: Vec<&str> = body.split('/').filter(|part| !part.is_empty()).collect();
        match parts.as_slice() {
            [id] => Some(Self {
                project: None,
                id: (*id).to_owned(),
            }),
            [project, id] => Some(Self {
                project: Some((*project).to_owned()),
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
        out.push_str(&self.id);
        out
    }

    /// The full name, filling the project in from whoever is asking.
    #[must_use]
    pub fn against(&self, asker: &Identity) -> Identity {
        Identity {
            project: self
                .project
                .clone()
                .unwrap_or_else(|| asker.project.clone()),
            id: self.id.clone(),
        }
    }
}

/// The directory a project's sockets live in.
#[must_use]
pub fn home(project: &str) -> PathBuf {
    runtime().join("axon").join(safe(project))
}

/// Where a socket for `me` is put, so an instance can be reached by name.
#[must_use]
pub fn listening_at(me: &Identity) -> PathBuf {
    home(&me.project).join(safe(&me.id))
}

/// Where the note saying who started `me` is put.
#[must_use]
pub fn kin_at(me: &Identity) -> PathBuf {
    home(&me.project).join(format!("{}.parent", safe(&me.id)))
}

/// The directory sockets live in.
fn runtime() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Flatten one name into one path segment.
///
/// A project is the working directory's name and a directory can be called anything, including
/// `..`. Anything that is not a letter, digit, dash or underscore becomes a dash, so a project
/// called `../../etc` cannot name a directory outside the one this chose.
fn safe(name: &str) -> String {
    let flattened: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // A name of nothing would name the parent directory itself.
    if flattened.is_empty() {
        "-".to_owned()
    } else {
        flattened
    }
}

/// Leave the note saying who started this session, so the tree can be read off the directory.
///
/// A main writes none, and that absence is what says it is one.
pub fn announce(me: &Identity) {
    let Some(parent) = parent() else { return };
    let path = kin_at(me);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, parent);
}

/// Take the note back down.
pub fn forget(me: &Identity) {
    let _ = std::fs::remove_file(kin_at(me));
}

/// What is known about a session in `project`, read off the directory.
///
/// Read rather than asked, so a session cannot describe its own place in the tree. The answer is
/// the same whether it is running, busy, or wedged.
#[must_use]
pub fn whom(project: &str, id: &str) -> Whom {
    let kin = home(project).join(format!("{}.parent", safe(id)));
    Whom {
        project: project.to_owned(),
        id: id.to_owned(),
        parent: std::fs::read_to_string(kin)
            .ok()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
    }
}

/// Every session currently listening in `project`.
///
/// Read from the directory rather than from a registry somebody has to keep up to date: a
/// process that died did not get to remove itself from a list, and a socket file that nothing
/// answers is discovered on the first call rather than trusted forever.
///
/// Only this project's, because there is no argument for any other and no way to ask for one.
#[must_use]
pub fn listening(project: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(home(project)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        // The `.parent` notes sit beside the sockets. An id is two Greek words and a dash, so
        // anything with a dot in it is not one.
        .filter(|name| !name.contains('.') && !name.is_empty())
        .collect();
    out.sort();
    out
}

/// Everyone `me` may actually reach, with how they stand to it.
///
/// The list the model is shown. Filtered here rather than at the point of calling, so a session
/// is never told about something it would then be refused — which reads as a broken tool.
#[must_use]
pub fn reachable(me: &Whom) -> Vec<(Whom, policy::Relation)> {
    listening(&me.project)
        .into_iter()
        .filter(|id| *id != me.id)
        .map(|id| {
            let them = whom(&me.project, &id);
            let relation = policy::between(me, &them);
            (them, relation)
        })
        .filter(|(_, relation)| policy::may(me, *relation, Reach::Ask))
        .collect()
}

/// Whether a path is one this process may listen on.
///
/// Belt and braces against [`safe`]: a socket path is built from a name that came off a wire,
/// and a name is not a promise. Two levels below the runtime root and no more, so neither half
/// can climb.
#[must_use]
pub fn inside(path: &Path) -> bool {
    let root = runtime().join("axon");
    path.parent()
        .and_then(Path::parent)
        .is_some_and(|grand| grand == root)
        && path
            .components()
            .all(|part| part.as_os_str() != std::ffi::OsStr::new(".."))
}

/// A name parses, resolves, and cannot escape the project it belongs to.
#[cfg(test)]
mod tests {
    use super::*;

    fn asker() -> Identity {
        Identity {
            project: "axon".to_owned(),
            id: "alpha-rho".to_owned(),
        }
    }

    #[test]
    fn a_bare_name_is_somebody_in_this_project() {
        let address = Address::read("$iota-mu").expect("an address");
        assert_eq!(address.id, "iota-mu");
        assert_eq!(address.against(&asker()).full(), "axon/iota-mu");
    }

    #[test]
    fn a_two_part_name_gives_the_project() {
        let address = Address::read("$other/beta-nu").expect("an address");
        assert_eq!(address.project.as_deref(), Some("other"));
        assert_eq!(address.against(&asker()).full(), "other/beta-nu");
    }

    #[test]
    fn the_sigil_is_optional_so_a_trigger_token_reads_the_same() {
        assert_eq!(Address::read("iota-mu"), Address::read("$iota-mu"));
    }

    #[test]
    fn an_address_that_names_nobody_is_not_an_address() {
        assert_eq!(Address::read("$"), None);
        assert_eq!(Address::read(""), None);
    }

    #[test]
    fn the_old_three_part_shape_is_not_read_as_a_name() {
        // `project/role/id` was the shape before roles left names. Reading it now would resolve
        // to `role/id` or `project/id` and reach a session that is not the one it meant.
        assert_eq!(Address::read("$axon/main/delta"), None);
    }

    #[test]
    fn what_was_written_comes_back_out() {
        for written in ["$iota-mu", "$other/beta-nu"] {
            let address = Address::read(written).expect("an address");
            assert_eq!(address.written(), written);
        }
    }

    #[test]
    fn a_socket_is_named_by_the_id_and_nothing_else() {
        let path = listening_at(&asker());
        assert_eq!(path.file_name().expect("a name"), "alpha-rho");
        assert_eq!(
            path.parent()
                .expect("a project")
                .file_name()
                .expect("a name"),
            "axon"
        );
    }

    #[test]
    fn each_project_gets_its_own_directory() {
        // The project wall, put in the filesystem: another project's sessions are not refused,
        // they are somewhere this one never lists.
        let mine = home("axon");
        let theirs = home("other");
        assert_ne!(mine, theirs);
        assert_eq!(mine.parent(), theirs.parent());
    }

    #[test]
    fn a_project_name_cannot_climb_out_of_the_runtime_directory() {
        // A project is the working directory's name, and a directory can be called `..`.
        let escaping = Identity {
            project: "../../etc".to_owned(),
            id: "../passwd".to_owned(),
        };
        let path = listening_at(&escaping);
        assert!(inside(&path), "it escaped to {path:?}");
        assert!(!path.to_string_lossy().contains(".."), "{path:?}");
    }

    #[test]
    fn a_name_of_nothing_does_not_name_the_directory_above() {
        assert_eq!(safe(""), "-");
        assert_eq!(safe("///"), "---");
    }

    #[test]
    fn the_parent_note_sits_beside_the_socket_and_is_not_mistaken_for_one() {
        let kin = kin_at(&asker());
        assert_eq!(kin.parent(), listening_at(&asker()).parent());
        let name = kin
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .into_owned();
        assert!(name.contains('.'), "{name} would be listed as a session");
    }

    #[test]
    fn a_session_with_no_note_beside_it_is_a_main() {
        // Absence is the whole of what makes one, so the fallback has to be that and not an
        // error: a directory that cannot be read must not turn a main into a subagent.
        let unknown = whom("no-such-project-here", "iota-mu");
        assert!(unknown.is_main());
    }

    #[test]
    fn listening_answers_nothing_rather_than_failing_with_no_directory() {
        // Nothing has started here yet, which is every project until something does.
        assert!(listening("no-such-project-here").is_empty());
    }
}
