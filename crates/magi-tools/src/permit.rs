//! Deciding whether a tool may do what it is about to do. An action outside what has already been
//! allowed stops and asks; the answer becomes a standing [`Grant`], or nothing when the person said
//! "just this once". The ledger holds grants for the session and the ones marked `Always` are also
//! written down. This module decides. It does not ask: that needs a UI on the other end of a socket.

pub use magi_proto::permit::{Action, Decision, Grant, Lifetime, Scope};

/// Every standing permission this session has.
#[derive(Debug, Default, Clone)]
pub struct Ledger {
    grants: Vec<Grant>,
}

impl Ledger {
    /// A ledger that has been told nothing, and so allows nothing without asking.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A ledger holding grants a configuration already made.
    #[must_use]
    pub fn with(grants: Vec<Grant>) -> Self {
        Self { grants }
    }

    /// Take on grants somebody else already holds, for a session that has been adopted: it may do
    /// what its parent may do, and no more. Added rather than replacing, so the child keeps what it
    /// was already allowed.
    pub fn take_on(&mut self, grants: Vec<Grant>) {
        self.grants.extend(grants);
    }

    /// Whether `action` is already covered, so nobody need be asked.
    #[must_use]
    pub fn allows(&self, action: &Action) -> bool {
        self.grants.iter().any(|g| g.covers(action))
    }

    /// Record what was decided about `action`. A refusal records nothing — it is an answer about
    /// this call, not a standing rule — and `Once` records nothing either, being spent on the call.
    pub fn remember(&mut self, action: &Action, decision: &Decision) {
        let Decision::Allow { scope, .. } = decision else {
            return;
        };
        let Some(grant) = standing(action, scope) else {
            return;
        };
        if !self.grants.contains(&grant) {
            self.grants.push(grant);
        }
    }

    /// The grants worth writing down.
    #[must_use]
    pub fn persistent(&self) -> &[Grant] {
        &self.grants
    }

    /// The widths a person should be offered for this action, narrow first, because the list is
    /// read top to bottom and the safest answer should be under the cursor.
    #[must_use]
    pub fn offers(action: &Action) -> Vec<Scope> {
        let mut out = vec![Scope::Once];
        match action {
            Action::Read { path } | Action::Write { path } => {
                out.push(Scope::Directory { path: path.clone() });
                if let Some(parent) = parent_of(path) {
                    out.push(Scope::Directory { path: parent });
                }
            }
            Action::Run { program, .. } => {
                out.push(Scope::Program {
                    program: program.clone(),
                });
            }
            Action::Network { host } => {
                out.push(Scope::Directory { path: host.clone() });
            }
        }
        out.push(Scope::Anything);
        out
    }
}

/// The grant a decision leaves behind, if any. `Exact` and `Once` are not stored as themselves: a
/// grant has to answer a future action, so an exact file becomes a directory of one, matching that
/// path and nothing under it. Public because the UI turns a person's answer into a grant the same
/// way, and a session lending its permissions to a child has to know what it holds.
pub fn standing(action: &Action, scope: &Scope) -> Option<Grant> {
    let verb = action.verb().to_owned();
    match scope {
        Scope::Once => None,
        Scope::Anything => Some(Grant {
            verb,
            scope: Scope::Anything,
        }),
        Scope::Directory { path } => Some(Grant {
            verb,
            scope: Scope::Directory { path: path.clone() },
        }),
        Scope::Program { program } => Some(Grant {
            verb,
            scope: Scope::Program {
                program: program.clone(),
            },
        }),
        Scope::Exact => match action {
            Action::Read { path } | Action::Write { path } => Some(Grant {
                verb,
                scope: Scope::Directory { path: path.clone() },
            }),
            // Nothing is stored: the only thing a `Run` grant can hold is a program, so "exactly
            // this" about a command line has no width here. It is spent on the call that asked.
            Action::Run { .. } => None,
            Action::Network { host } => Some(Grant {
                verb,
                scope: Scope::Directory { path: host.clone() },
            }),
        },
    }
}

/// The directory holding `path`.
fn parent_of(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let at = trimmed.rfind('/')?;
    if at == 0 {
        return Some("/".to_owned());
    }
    Some(trimmed[..at].to_owned())
}

/// Whether a lifetime should reach the file on disk.
#[must_use]
pub fn is_persistent(lifetime: Lifetime) -> bool {
    matches!(lifetime, Lifetime::Always)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &str) -> Action {
        Action::Read {
            path: path.to_owned(),
        }
    }

    fn run(command: &str) -> Action {
        Action::Run {
            command: command.to_owned(),
            program: command.split_whitespace().next().unwrap_or("").to_owned(),
        }
    }

    fn allow(scope: Scope) -> Decision {
        Decision::Allow {
            scope,
            lifetime: Lifetime::Session,
        }
    }

    #[test]
    fn a_new_ledger_allows_nothing() {
        // The default has to be "ask": opt-in safety is off for everybody it was meant to protect.
        assert!(!Ledger::new().allows(&read("/etc/passwd")));
    }

    #[test]
    fn a_directory_answer_covers_the_rest_of_the_directory() {
        let mut ledger = Ledger::new();
        ledger.remember(
            &read("/home/x/work/a.rs"),
            &allow(Scope::Directory {
                path: "/home/x/work".to_owned(),
            }),
        );
        assert!(ledger.allows(&read("/home/x/work/b.rs")));
        assert!(!ledger.allows(&read("/home/x/other/b.rs")));
    }

    #[test]
    fn once_leaves_nothing_behind() {
        let mut ledger = Ledger::new();
        ledger.remember(&run("git status"), &allow(Scope::Once));
        assert!(!ledger.allows(&run("git status")), "asked again next time");
    }

    #[test]
    fn a_refusal_leaves_nothing_behind() {
        // A "no" is an answer about this call, not a rule to be found and undone later.
        let mut ledger = Ledger::new();
        ledger.remember(&run("rm -rf /"), &Decision::Deny);
        assert!(ledger.persistent().is_empty());
    }

    #[test]
    fn an_exact_file_does_not_become_its_directory() {
        // The narrow answer has to stay narrow: `Exact` on a file matches that path and nothing
        // else in the folder.
        let mut ledger = Ledger::new();
        ledger.remember(&read("/home/x/work/a.rs"), &allow(Scope::Exact));
        assert!(ledger.allows(&read("/home/x/work/a.rs")));
        assert!(!ledger.allows(&read("/home/x/work/b.rs")));
    }

    #[test]
    fn a_program_answer_covers_that_program_with_any_arguments() {
        let mut ledger = Ledger::new();
        ledger.remember(
            &run("git status"),
            &allow(Scope::Program {
                program: "git".to_owned(),
            }),
        );
        assert!(ledger.allows(&run("git log -p")));
        assert!(!ledger.allows(&run("curl example.com")));
    }

    #[test]
    fn the_same_answer_twice_is_one_grant() {
        let mut ledger = Ledger::new();
        for _ in 0..3 {
            ledger.remember(
                &run("git status"),
                &allow(Scope::Program {
                    program: "git".to_owned(),
                }),
            );
        }
        assert_eq!(ledger.persistent().len(), 1);
    }

    #[test]
    fn a_file_is_offered_its_own_directory_and_its_parent() {
        // Fine to broad, in one prompt: this file, this folder, everything.
        let offers = Ledger::offers(&read("/home/x/work/src/a.rs"));
        assert!(matches!(offers.first(), Some(Scope::Once)));
        assert!(matches!(offers.last(), Some(Scope::Anything)));
        let labels: Vec<String> = offers.iter().map(|s| s.label(&read("/x"))).collect();
        assert!(
            labels.iter().any(|l| l.contains("/home/x/work/src")),
            "the folder is offered: {labels:?}"
        );
    }

    #[test]
    fn a_command_is_offered_its_program() {
        let offers = Ledger::offers(&run("git status --short"));
        assert!(
            offers
                .iter()
                .any(|s| matches!(s, Scope::Program { program } if program == "git")),
            "{offers:?}"
        );
    }

    #[test]
    fn configured_grants_are_honoured_without_asking() {
        // A person who wrote the rule down has already answered the question.
        let ledger = Ledger::with(vec![Grant {
            verb: "read".to_owned(),
            scope: Scope::Anything,
        }]);
        assert!(ledger.allows(&read("/anywhere")));
    }

    #[test]
    fn a_parent_of_a_top_level_path_is_the_root() {
        assert_eq!(parent_of("/etc"), Some("/".to_owned()));
        assert_eq!(parent_of("/"), None);
    }
}
