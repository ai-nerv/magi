//! What a tool wants to do, and what it has been allowed to do.
//!
//! On the wire because both ends need it: the daemon asks, the UI answers, and a grant the
//! person made is a fact the daemon has to be told rather than something it can work out.
//!
//! **The shape of the question.** A tool does not ask "may I run?" — it says what it is about
//! to do, in terms a person can judge without reading the tool's source. Reading a file is a
//! different question from writing one, and running `git status` is a different question from
//! running `rm -rf`, even though both are "the shell".
//!
//! **The shape of the answer.** One request can be answered at several widths, because how much
//! you want to grant depends on what was asked. Saying yes to *this exact command, once* and
//! saying yes to *anything under this directory, forever* are both reasonable answers to the
//! same prompt, and a system that offers only one of them will be answered carelessly.

use serde::{Deserialize, Serialize};

/// Something a tool is about to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Action {
    /// Read a path.
    Read {
        /// The path, as the tool resolved it.
        path: String,
    },
    /// Create, overwrite or modify a path.
    Write {
        /// The path, as the tool resolved it.
        path: String,
    },
    /// Run a command in a shell.
    Run {
        /// The command line, verbatim.
        command: String,
        /// Its first word, which is what a person judges it by.
        program: String,
    },
    /// Reach a host over the network.
    Network {
        /// The host, as the tool named it.
        host: String,
    },
}

impl Action {
    /// The verb, for a prompt.
    #[must_use]
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Read { .. } => "read",
            Self::Write { .. } => "write",
            Self::Run { .. } => "run",
            Self::Network { .. } => "reach",
        }
    }

    /// What it is about to act on, for a prompt.
    #[must_use]
    pub fn subject(&self) -> &str {
        match self {
            Self::Read { path } | Self::Write { path } => path,
            Self::Run { command, .. } => command,
            Self::Network { host } => host,
        }
    }
}

/// How widely an answer applies.
///
/// Ordered from narrowest to widest, and a person is offered several of them at once: the width
/// they want depends on what was asked, and a prompt that offers only "yes" trains them to press
/// yes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Scope {
    /// This exact action, this once.
    Once,
    /// This exact path, or this exact command line.
    Exact,
    /// Anything under a directory.
    Directory {
        /// The directory, without a trailing separator.
        path: String,
    },
    /// Any command beginning with this program.
    Program {
        /// The first word of the command line.
        program: String,
    },
    /// Every action of this kind, anywhere.
    Anything,
}

impl Scope {
    /// How it reads in a menu.
    #[must_use]
    pub fn label(&self, action: &Action) -> String {
        match self {
            Self::Once => "just this once".to_owned(),
            Self::Exact => match action {
                Action::Read { .. } | Action::Write { .. } => "this file, from now on".to_owned(),
                Action::Run { .. } => "this exact command, from now on".to_owned(),
                Action::Network { .. } => "this host, from now on".to_owned(),
            },
            // A directory whose path *is* what was asked about is the file itself, and
            // "anything under /etc/hostname" reads as a wider grant than it is.
            Self::Directory { path } if path == action.subject() => match action {
                Action::Read { .. } | Action::Write { .. } => "just this file".to_owned(),
                _ => format!("just {path}"),
            },
            Self::Directory { path } => format!("anything under {path}"),
            Self::Program { program } => format!("any `{program}` command"),
            Self::Anything => format!("anything this session can {}", action.verb()),
        }
    }
}

/// How long an answer lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    /// Until this session ends.
    Session,
    /// Written down, and honoured next time.
    Always,
}

/// What was decided about one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Decision {
    /// Allowed, at this width, for this long.
    Allow {
        /// How widely it applies.
        scope: Scope,
        /// How long it lasts.
        lifetime: Lifetime,
    },
    /// Refused. The tool is told, and the model reads it as a result.
    Deny,
}

/// A standing permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Which verb it covers.
    pub verb: String,
    /// How widely.
    pub scope: Scope,
}

impl Grant {
    /// Whether this grant answers `action`.
    #[must_use]
    pub fn covers(&self, action: &Action) -> bool {
        if self.verb != action.verb() {
            return false;
        }
        match &self.scope {
            // Never stored: it was spent on the call that asked.
            Scope::Once => false,
            Scope::Anything => true,
            Scope::Exact => false,
            // A host belongs here as well as a path. `offers` has always produced a
            // `Directory` for `Action::Network` — a socket lives at a path, and "any socket
            // under this directory" is the useful width — but this arm answered only about
            // reads and writes, so every network grant ever made was inert and the person was
            // asked again on the next connect.
            Scope::Directory { path } => match action {
                Action::Read { path: at }
                | Action::Write { path: at }
                | Action::Network { host: at } => under(at, path),
                _ => false,
            },
            Scope::Program { program } => match action {
                Action::Run { program: p, .. } => p == program,
                _ => false,
            },
        }
    }
}

/// Whether `path` is inside `root`.
///
/// Textual, on already-resolved paths: the tool has normalised its argument before asking, and a
/// grant that re-resolved it could answer about a different file from the one being opened.
#[must_use]
pub fn under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return true;
    }
    path == root || path.starts_with(&format!("{root}/"))
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

    #[test]
    fn a_directory_grant_covers_what_is_under_it() {
        let grant = Grant {
            verb: "read".to_owned(),
            scope: Scope::Directory {
                path: "/home/x/work".to_owned(),
            },
        };
        assert!(grant.covers(&read("/home/x/work/src/main.rs")));
        assert!(grant.covers(&read("/home/x/work")));
        assert!(!grant.covers(&read("/home/x/secrets")));
    }

    #[test]
    fn a_directory_grant_is_not_fooled_by_a_shared_prefix() {
        // `/home/x/work-secrets` is not under `/home/x/work`.
        let grant = Grant {
            verb: "read".to_owned(),
            scope: Scope::Directory {
                path: "/home/x/work".to_owned(),
            },
        };
        assert!(!grant.covers(&read("/home/x/work-secrets/f")));
    }

    #[test]
    fn a_network_grant_answers_a_network_action() {
        // It did not, for as long as `Action::Network` existed. `offers` produced a `Directory`
        // for a host and `covers` matched `Directory` only against a read or a write, so the
        // grant was written down, looked right in the ledger, and answered nothing.
        let grant = Grant {
            verb: "reach".to_owned(),
            scope: Scope::Directory {
                path: "/run/user/1000/magi".to_owned(),
            },
        };
        assert!(grant.covers(&Action::Network {
            host: "/run/user/1000/magi/casper.sock".to_owned(),
        }));
        assert!(!grant.covers(&Action::Network {
            host: "/run/user/1000/other/casper.sock".to_owned(),
        }));
    }

    #[test]
    fn a_grant_does_not_cross_verbs() {
        // Being allowed to read a directory is not being allowed to write in it.
        let grant = Grant {
            verb: "read".to_owned(),
            scope: Scope::Anything,
        };
        assert!(!grant.covers(&Action::Write {
            path: "/anything".to_owned()
        }));
    }

    #[test]
    fn a_program_grant_covers_that_program_only() {
        let grant = Grant {
            verb: "run".to_owned(),
            scope: Scope::Program {
                program: "git".to_owned(),
            },
        };
        assert!(grant.covers(&run("git status --short")));
        assert!(!grant.covers(&run("rm -rf /")));
    }

    #[test]
    fn once_is_never_a_standing_grant() {
        // It was spent on the call that asked; storing it would make a single yes permanent.
        let grant = Grant {
            verb: "run".to_owned(),
            scope: Scope::Once,
        };
        assert!(!grant.covers(&run("git status")));
    }

    #[test]
    fn exact_is_not_matched_here_because_it_needs_the_subject() {
        // `Exact` is stored as the narrowest thing that *does* carry its subject — a directory
        // of one file, or a program of one word — rather than as a variant that has to be
        // compared against the original request.
        let grant = Grant {
            verb: "read".to_owned(),
            scope: Scope::Exact,
        };
        assert!(!grant.covers(&read("/anything")));
    }

    #[test]
    fn a_label_says_what_it_is_granting() {
        let action = run("git status");
        assert!(Scope::Once.label(&action).contains("once"));
        assert!(
            Scope::Program {
                program: "git".to_owned()
            }
            .label(&action)
            .contains("git")
        );
    }

    #[test]
    fn a_root_of_nothing_covers_everything_rather_than_nothing() {
        // An empty root is the filesystem root once the trailing separator is trimmed.
        assert!(under("/anywhere", "/"));
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    fn read(path: &str) -> Action {
        Action::Read {
            path: path.to_owned(),
        }
    }

    #[test]
    fn the_file_itself_does_not_read_as_a_directory() {
        // "anything under /etc/hostname" reads as a wider grant than it is.
        let action = read("/etc/hostname");
        let scope = Scope::Directory {
            path: "/etc/hostname".to_owned(),
        };
        assert_eq!(scope.label(&action), "just this file");
    }

    #[test]
    fn a_real_directory_still_says_so() {
        let action = read("/etc/hostname");
        let scope = Scope::Directory {
            path: "/etc".to_owned(),
        };
        assert_eq!(scope.label(&action), "anything under /etc");
    }

    #[test]
    fn a_host_that_is_the_subject_is_named_rather_than_described() {
        let action = Action::Network {
            host: "example.com".to_owned(),
        };
        let scope = Scope::Directory {
            path: "example.com".to_owned(),
        };
        assert_eq!(scope.label(&action), "just example.com");
    }
}
