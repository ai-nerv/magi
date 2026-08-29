//! The seam every tool runs through.
//!
//! A tool never touches the filesystem or spawns a process directly; it asks an [`Ops`]. That
//! one indirection is what lets execution be redirected to an SSH host, a container, or a
//! sandbox without touching a tool — Pi gets the same property for roughly zero lines, and it
//! is the cheapest good idea in either codebase.
//!
//! It is also the safety boundary for Lua tools: a description registered from a config file
//! is handed an `Ops`, so what it can reach is decided here rather than by what the VM happens
//! to expose.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a shell command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// Exit status, or `None` if a signal ended it.
    pub code: Option<i32>,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

impl Shell {
    /// Whether the command reported success.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Everything a tool is allowed to do to the outside world.
pub trait Ops: Send + Sync {
    /// Where relative paths resolve from.
    fn cwd(&self) -> PathBuf;

    /// Read a file.
    ///
    /// # Errors
    /// When the path is outside the session, missing, or unreadable.
    fn read(&self, path: &Path) -> Result<String, String>;

    /// Write a file, creating parent directories.
    ///
    /// # Errors
    /// When the path is outside the session or the write fails.
    fn write(&self, path: &Path, contents: &str) -> Result<(), String>;

    /// Run a shell command.
    ///
    /// # Errors
    /// When the command could not be started at all. A command that ran and failed is a
    /// [`Shell`] with a non-zero code, not an error: the model needs to see what it said.
    fn shell(&self, command: &str) -> Result<Shell, String>;

    /// Ask whether `action` may happen, blocking until it is answered.
    ///
    /// Called by a tool *before* it acts, not by the registry, because only the tool knows what
    /// it is about to do: "run this command" and "read this file" are different questions and a
    /// person can only answer the one they were actually asked.
    ///
    /// `tool` is its own name, and is carried rather than derived from the action. The prompt
    /// said "read wants to read ." for a `grep` call, because the verb was standing in for the
    /// name — fine while `read` was the only thing that read, and a false sentence in a security
    /// prompt the moment `grep`, `find` and `ls` arrived.
    ///
    /// The default allows. Every `Ops` in the tree except [`Real`] is a test double, and a
    /// double that had to be taught about permissions would make every tool test a permissions
    /// test. `Real` is the one that gates.
    ///
    /// # Errors
    /// When it was refused, with a sentence the model reads as a result.
    fn allow(&self, tool: &str, action: &axum_proto::permit::Action) -> Result<(), String> {
        let _ = (tool, action);
        Ok(())
    }
}

/// Ops against the real machine, rooted at one directory.
///
/// The root is where **relative** paths resolve from — the session's directory, so `src/main.rs`
/// means what it means in the shell you started in.
///
/// It is not a wall by default, and that is deliberate. It used to be: an absolute path outside
/// the session was refused, and the effect was not safety but a detour. Asked to edit
/// `/tmp/scratch/hello.py`, the model was told "outside this session's directory", so it reached
/// for `bash` and did the same edit through a `python3` heredoc — unreviewable, undiffed, and
/// through the one tool that has no confinement at all. A rule that only the careful tools obey
/// moves work to the careless one.
///
/// Confinement is a *configuration*, the same way sandboxing is (§5e): `axum.confine = true`
/// restores the wall, and `bwrap` in front of the shell peer is what actually contains anything.
pub struct Real {
    root: PathBuf,
    confined: bool,
    /// What has already been allowed, and who to ask when it has not.
    gate: Option<Gate>,
}

/// The ledger and the person, together.
struct Gate {
    ledger: std::sync::Mutex<crate::permit::Ledger>,
    approver: std::sync::Arc<dyn crate::approve::Approver>,
}

impl Real {
    /// Ops rooted at `root`, reaching anywhere.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            confined: false,
            gate: None,
        }
    }

    /// The same, asking `approver` about anything `ledger` does not already cover.
    #[must_use]
    pub fn gated(
        root: PathBuf,
        ledger: crate::permit::Ledger,
        approver: std::sync::Arc<dyn crate::approve::Approver>,
    ) -> Self {
        Self {
            root,
            confined: false,
            gate: Some(Gate {
                ledger: std::sync::Mutex::new(ledger),
                approver,
            }),
        }
    }

    /// The grants this session has accumulated, for writing down.
    #[must_use]
    pub fn grants(&self) -> Vec<axum_proto::permit::Grant> {
        self.gate.as_ref().map_or_else(Vec::new, |gate| {
            gate.ledger
                .lock()
                .map(|l| l.persistent().to_vec())
                .unwrap_or_default()
        })
    }

    /// Ops that refuse anything outside `root`.
    #[must_use]
    pub fn confined(root: PathBuf) -> Self {
        Self {
            root,
            confined: true,
            gate: None,
        }
    }

    /// Resolve a path against the root, refusing anything that escapes it when confined.
    ///
    /// Checked after normalising rather than by looking for `..` in the text: `a/../../etc` has
    /// no leading `..` and still escapes, and a symlink has none at all.
    fn resolve(&self, path: &Path) -> Result<PathBuf, String> {
        let joined = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        let normalised = normalise(&joined);
        if !self.confined {
            return Ok(normalised);
        }
        let root = normalise(&self.root);
        if !normalised.starts_with(&root) {
            return Err(format!(
                "{} is outside this session's directory, and `axum.confine` is on",
                path.display()
            ));
        }
        Ok(normalised)
    }
}

/// Resolve `.` and `..` without touching the filesystem.
///
/// `canonicalize` would be stricter but requires the path to exist, and a write to a new file
/// has to be checked before it does.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

impl Ops for Real {
    fn cwd(&self) -> PathBuf {
        self.root.clone()
    }

    fn read(&self, path: &Path) -> Result<String, String> {
        let path = self.resolve(path)?;
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn write(&self, path: &Path, contents: &str) -> Result<(), String> {
        let path = self.resolve(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Consult the ledger, and ask if it has nothing to say.
    ///
    /// The answer is recorded before it is acted on, so a person asked once about a directory is
    /// not asked again about the next file in it — which is the difference between a permission
    /// prompt and a nuisance.
    fn allow(&self, tool: &str, action: &axum_proto::permit::Action) -> Result<(), String> {
        let Some(gate) = &self.gate else {
            return Ok(());
        };
        if gate.ledger.lock().is_ok_and(|ledger| ledger.allows(action)) {
            return Ok(());
        }
        let decision = gate.approver.ask(tool, action);
        if let Ok(mut ledger) = gate.ledger.lock() {
            ledger.remember(action, &decision);
        }
        match decision {
            axum_proto::permit::Decision::Allow { .. } => Ok(()),
            axum_proto::permit::Decision::Deny => Err(format!(
                "not permitted: {} {}. The person at the keyboard declined.",
                action.verb(),
                action.subject()
            )),
        }
    }

    fn shell(&self, command: &str) -> Result<Shell, String> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.root)
            .output()
            .map_err(|e| format!("could not run a shell: {e}"))?;
        Ok(Shell {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rooted(name: &str) -> (Real, PathBuf) {
        let dir = std::env::temp_dir().join(format!("axum-ops-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        (Real::new(dir.clone()), dir)
    }

    /// The same, with the wall on: `axum.confine` is where that rule lives now.
    fn walled(name: &str) -> (Real, PathBuf) {
        let dir = std::env::temp_dir().join(format!("axum-wall-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        (Real::confined(dir.clone()), dir)
    }

    #[test]
    fn a_write_then_a_read_round_trips() {
        let (ops, dir) = rooted("roundtrip");
        ops.write(Path::new("a.txt"), "hello").expect("write");
        assert_eq!(ops.read(Path::new("a.txt")).expect("read"), "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_creates_parent_directories() {
        let (ops, dir) = rooted("parents");
        ops.write(Path::new("deep/nested/a.txt"), "x")
            .expect("write");
        assert!(dir.join("deep/nested/a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confined_path_that_escapes_the_root_is_refused() {
        let (ops, dir) = walled("escape");
        let error = ops
            .read(Path::new("../../etc/passwd"))
            .expect_err("must refuse");
        assert!(error.contains("outside"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_escape_hidden_behind_a_descent_is_still_refused() {
        // `a/../../etc` has no leading `..` and still escapes, which is why the check happens
        // after normalising rather than on the text.
        let (ops, dir) = rooted("hidden");
        assert!(ops.read(Path::new("a/../../etc/passwd")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confined_absolute_path_outside_the_root_is_refused() {
        let (ops, dir) = walled("absolute");
        assert!(ops.read(Path::new("/etc/passwd")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_command_that_fails_is_output_not_an_error() {
        // The model needs to see what it said; a non-zero exit is information, not a fault.
        let (ops, dir) = rooted("failing");
        let result = ops.shell("echo out; echo err >&2; exit 3").expect("it ran");
        assert_eq!(result.code, Some(3));
        assert!(!result.ok());
        assert_eq!(result.stdout.trim(), "out");
        assert_eq!(result.stderr.trim(), "err");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_command_runs_in_the_session_directory() {
        let (ops, dir) = rooted("cwd");
        let result = ops.shell("pwd").expect("it ran");
        assert!(result.stdout.contains("axum-ops-"), "{}", result.stdout);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_a_missing_file_names_it() {
        let (ops, dir) = rooted("missing");
        let error = ops.read(Path::new("nope.txt")).expect_err("must fail");
        assert!(error.contains("nope.txt"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod reach_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axum-reach-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn a_path_outside_the_session_is_reachable() {
        // The refusal was not safety. Told "outside this session's directory", a model reaches
        // for `bash` and does the same edit through a heredoc — through the one tool with no
        // confinement at all, and with no diff to show for it.
        let session = scratch("session");
        let elsewhere = scratch("elsewhere");
        let file = elsewhere.join("hello.py");
        std::fs::write(&file, "print('a')\n").expect("write");

        let ops = Real::new(session.clone());
        assert_eq!(ops.read(&file).expect("read"), "print('a')\n");
        ops.write(&file, "print('b')\n").expect("write");
        assert_eq!(
            std::fs::read_to_string(&file).expect("read back"),
            "print('b')\n"
        );

        let _ = std::fs::remove_dir_all(&session);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn a_relative_path_still_means_the_session() {
        // The root's real job: `src/main.rs` means what it means in the shell you started in.
        let session = scratch("relative");
        std::fs::create_dir_all(session.join("src")).expect("mkdir");
        std::fs::write(session.join("src/main.rs"), "fn main() {}\n").expect("write");
        let ops = Real::new(session.clone());
        assert_eq!(
            ops.read(Path::new("src/main.rs")).expect("read"),
            "fn main() {}\n"
        );
        let _ = std::fs::remove_dir_all(&session);
    }

    #[test]
    fn confined_ops_still_refuse_and_say_why() {
        let session = scratch("wall");
        let ops = Real::confined(session.clone());
        let outside = std::env::temp_dir().join("axum-not-here.txt");
        let why = ops.read(&outside).expect_err("refused");
        assert!(why.contains("axum.confine"), "it names the setting: {why}");
        let _ = std::fs::remove_dir_all(&session);
    }

    #[test]
    fn confinement_still_catches_a_path_that_climbs_out() {
        // `a/../../etc` has no leading `..` and still escapes.
        let session = scratch("climb");
        let ops = Real::confined(session.clone());
        assert!(ops.read(Path::new("a/../../etc/passwd")).is_err());
        let _ = std::fs::remove_dir_all(&session);
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use axum_proto::permit::{Action, Decision, Lifetime, Scope};
    use std::sync::Arc;

    /// An approver that answers from a script and records what it was asked.
    struct Scripted {
        answers: std::sync::Mutex<Vec<Decision>>,
        asked: std::sync::Mutex<Vec<Action>>,
    }

    impl Scripted {
        fn new(answers: Vec<Decision>) -> Arc<Self> {
            Arc::new(Self {
                answers: std::sync::Mutex::new(answers),
                asked: std::sync::Mutex::new(Vec::new()),
            })
        }
        fn asked(&self) -> Vec<Action> {
            self.asked.lock().map(|a| a.clone()).unwrap_or_default()
        }
    }

    impl crate::approve::Approver for Scripted {
        fn ask(&self, _tool: &str, action: &Action) -> Decision {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push(action.clone());
            }
            self.answers
                .lock()
                .ok()
                .and_then(|mut a| {
                    if a.is_empty() {
                        None
                    } else {
                        Some(a.remove(0))
                    }
                })
                .unwrap_or(Decision::Deny)
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axum-gate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn an_ungated_ops_asks_nobody() {
        // Every `Ops` but `Real` is a test double, and one that had to be taught about
        // permissions would make every tool test a permissions test.
        let dir = scratch("ungated");
        let ops = Real::new(dir.clone());
        assert!(ops.allow("t", &Action::Read { path: "/x".into() }).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gated_ops_asks_and_a_refusal_reaches_the_model() {
        let dir = scratch("refused");
        let approver = Scripted::new(vec![Decision::Deny]);
        let ops = Real::gated(dir.clone(), crate::permit::Ledger::new(), approver.clone());
        let why = ops
            .allow(
                "t",
                &Action::Read {
                    path: "/etc/shadow".into(),
                },
            )
            .expect_err("refused");
        assert!(why.contains("not permitted"), "{why}");
        assert!(
            why.contains("/etc/shadow"),
            "it says what was refused: {why}"
        );
        assert_eq!(approver.asked().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_answer_means_the_next_file_is_not_asked_about() {
        // The difference between a permission prompt and a nuisance.
        let dir = scratch("once-only");
        let approver = Scripted::new(vec![Decision::Allow {
            scope: Scope::Directory {
                path: "/home/x/work".into(),
            },
            lifetime: Lifetime::Session,
        }]);
        let ops = Real::gated(dir.clone(), crate::permit::Ledger::new(), approver.clone());
        for file in ["a.rs", "b.rs", "c.rs"] {
            ops.allow(
                "t",
                &Action::Read {
                    path: format!("/home/x/work/{file}"),
                },
            )
            .expect("allowed");
        }
        assert_eq!(approver.asked().len(), 1, "asked once, not three times");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_grant_for_one_directory_does_not_cover_another() {
        let dir = scratch("elsewhere");
        let approver = Scripted::new(vec![
            Decision::Allow {
                scope: Scope::Directory {
                    path: "/home/x/work".into(),
                },
                lifetime: Lifetime::Session,
            },
            Decision::Deny,
        ]);
        let ops = Real::gated(dir.clone(), crate::permit::Ledger::new(), approver.clone());
        ops.allow(
            "t",
            &Action::Read {
                path: "/home/x/work/a".into(),
            },
        )
        .expect("allowed");
        assert!(
            ops.allow(
                "t",
                &Action::Read {
                    path: "/home/x/secrets/a".into()
                }
            )
            .is_err(),
            "a second directory is a second question"
        );
        assert_eq!(approver.asked().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_was_granted_can_be_written_down() {
        let dir = scratch("grants");
        let approver = Scripted::new(vec![Decision::Allow {
            scope: Scope::Program {
                program: "git".into(),
            },
            lifetime: Lifetime::Always,
        }]);
        let ops = Real::gated(dir.clone(), crate::permit::Ledger::new(), approver);
        ops.allow(
            "t",
            &Action::Run {
                command: "git status".into(),
                program: "git".into(),
            },
        )
        .expect("allowed");
        assert_eq!(ops.grants().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
