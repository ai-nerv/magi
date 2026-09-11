//! The seam every tool runs through: a tool never touches the filesystem or spawns a process
//! directly, it asks an [`Ops`], so execution can be redirected to an SSH host, a container or a
//! sandbox without touching a tool. It is also the safety boundary for Lua tools, whose reach is
//! decided here rather than by what the VM happens to expose.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a shell command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    /// Exit status, or `None` if a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Shell {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Everything a tool is allowed to do to the outside world.
pub trait Ops: Send + Sync {
    /// Where relative paths resolve from.
    fn cwd(&self) -> PathBuf;

    /// The path a tool is about to act on, as the person should be asked about it: one string for
    /// the question and the deed, because a `Directory` grant is matched textually and two
    /// spellings of the same file would otherwise never meet. Lexical, like `resolve`, since a
    /// write to a new file has to be asked about before the file exists.
    fn resolved(&self, path: &Path) -> PathBuf {
        let joined = if path.is_absolute() {
            path.to_owned()
        } else {
            self.cwd().join(path)
        };
        normalise(&joined)
    }

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
    /// When the command could not be started at all. A command that ran and failed is a [`Shell`]
    /// with a non-zero code, not an error.
    fn shell(&self, command: &str) -> Result<Shell, String>;

    /// Ask whether `action` may happen, blocking until it is answered. Called by a tool before it
    /// acts, not by the registry, because only the tool knows what it is about to do; `tool` is its
    /// own name rather than the verb, so a `grep` call does not say "read wants to read .". The
    /// default allows — [`Real`] is the one that gates.
    ///
    /// # Errors
    /// When it was refused, with a sentence the model reads as a result.
    fn allow(&self, tool: &str, action: &magi_proto::permit::Action) -> Result<(), String> {
        let _ = (tool, action);
        Ok(())
    }

    /// Take on grants a parent session already holds. `&self`, because the ledger is behind a lock:
    /// this arrives while the session is running. Nothing by default.
    fn take_on(&self, grants: Vec<magi_proto::permit::Grant>) {
        let _ = grants;
    }

    /// Permission questions decided since the last time this was asked, read by the turn loop where
    /// the watchers are; see [`crate::watching::Pending`]. Nothing by default.
    fn noticed(&self) -> Vec<crate::watching::Noted> {
        Vec::new()
    }

    /// The jail profile a tools program is spawned with, as JSON; `None` when isolation is off.
    fn jail(&self) -> Option<String> {
        None
    }
}

/// Ops against the real machine, rooted at one directory. The root is where *relative* paths
/// resolve from — the session's directory, so `src/main.rs` means what it means in the shell you
/// started in. Not a wall by default: a rule only the careful tools obey moves work to `bash`.
/// Confinement is a configuration, `magi.confine = true`, and `bwrap` is what contains anything.
pub struct Real {
    root: PathBuf,
    confined: bool,
    /// Whether a tool command runs inside a kernel jail — `magi.isolation`. Independent of
    /// [`Self::confined`], the pre-flight path check: this contains a command that ignores it.
    isolate: bool,
    /// What has already been allowed, and who to ask when it has not.
    gate: Option<Gate>,
    /// Questions and answers, written down for whoever can be told; see [`crate::watching::Pending`].
    noticed: crate::watching::Pending,
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
            isolate: false,
            noticed: crate::watching::Pending::new(),
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
            isolate: false,
            noticed: crate::watching::Pending::new(),
        }
    }

    /// The grants this session has accumulated, for writing down.
    #[must_use]
    pub fn grants(&self) -> Vec<magi_proto::permit::Grant> {
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
            isolate: false,
            noticed: crate::watching::Pending::new(),
        }
    }

    /// Keep this one inside its root, or do not. A method rather than a fourth constructor:
    /// confinement and gating are independent, and the combinations grow by multiplication.
    #[must_use]
    pub fn confining(mut self, confined: bool) -> Self {
        self.confined = confined;
        self
    }

    /// Run tool commands inside a kernel jail, or do not.
    #[must_use]
    pub fn isolating(mut self, isolate: bool) -> Self {
        self.isolate = isolate;
        self
    }

    /// What the jail may write, and whether it may reach the network, from this session's grants: a
    /// write grant on a directory makes it writable, any reach grant keeps the network. The one
    /// reading of the ledger both the tools-program profile and `magi.shell` are built from.
    fn jail_reach(&self) -> (Vec<PathBuf>, bool) {
        use magi_proto::permit::Scope;
        let mut write = Vec::new();
        let mut reach = false;
        for grant in self.grants() {
            match (grant.verb.as_str(), &grant.scope) {
                ("write", Scope::Directory { path }) => write.push(PathBuf::from(path)),
                ("reach", _) => reach = true,
                _ => {}
            }
        }
        (write, reach)
    }

    /// Resolve a path against the root, refusing anything that escapes it when confined. Checked
    /// after normalising rather than on the text: `a/../../etc` has no leading `..` and a symlink
    /// has none at all.
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
                "{} is outside this session's directory, and `magi.confine` is on",
                path.display()
            ));
        }
        Ok(normalised)
    }
}

/// Resolve `.` and `..` without touching the filesystem. `canonicalize` would be stricter but
/// requires the path to exist, and a write to a new file has to be checked before it does.
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

    /// Consult the ledger, and ask if it has nothing to say. The answer is recorded before it is
    /// acted on, so a person asked once about a directory is not asked again about the next file.
    fn allow(&self, tool: &str, action: &magi_proto::permit::Action) -> Result<(), String> {
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
        self.noticed.note(crate::watching::Noted {
            verb: action.verb().to_string(),
            about: action.subject().to_string(),
            allowed: matches!(decision, magi_proto::permit::Decision::Allow { .. }),
        });
        match decision {
            magi_proto::permit::Decision::Allow { .. } => Ok(()),
            magi_proto::permit::Decision::Deny => Err(format!(
                "not permitted: {} {}. The person at the keyboard declined.",
                action.verb(),
                action.subject()
            )),
        }
    }

    fn noticed(&self) -> Vec<crate::watching::Noted> {
        self.noticed.drain()
    }

    /// The jail profile from the grants this session holds. `None` when isolation is off; otherwise
    /// the write directories and network toggle, conservative on an empty ledger, not open.
    fn jail(&self) -> Option<String> {
        if !self.isolate {
            return None;
        }
        let (write, reach) = self.jail_reach();
        serde_json::to_string(&serde_json::json!({ "write": write, "reach": reach })).ok()
    }

    fn take_on(&self, grants: Vec<magi_proto::permit::Grant>) {
        if let Some(gate) = &self.gate
            && let Ok(mut ledger) = gate.ledger.lock()
        {
            ledger.take_on(grants);
        }
    }

    fn shell(&self, command: &str) -> Result<Shell, String> {
        // Jailed when this session runs sandboxed, from the same grants the profile is built from,
        // so `magi.shell` is contained exactly as a tool command is. `sh -c` directly otherwise.
        let mut spawning = if self.isolate {
            let (write, reach) = self.jail_reach();
            crate::jail::shell(command, &self.root, &write, reach)
        } else {
            let mut sh = Command::new("sh");
            sh.arg("-c").arg(command).current_dir(&self.root);
            sh
        };
        let output = spawning
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
    use magi_model::scratch::Scratch;

    fn rooted(name: &str) -> (Real, Scratch) {
        let dir = Scratch::new("magi-ops", name);
        (Real::new(dir.to_path_buf()), dir)
    }

    #[test]
    fn isolation_off_is_no_jail_profile_at_all() {
        let (ops, _dir) = rooted("no-jail");
        assert_eq!(
            ops.jail(),
            None,
            "a session that did not ask for a jail gets none"
        );
    }

    #[test]
    fn the_jail_profile_is_built_from_the_grants() {
        use magi_proto::permit::{Grant, Scope};
        let ledger = crate::permit::Ledger::with(vec![
            Grant {
                verb: "write".to_owned(),
                scope: Scope::Directory {
                    path: "/w/build".to_owned(),
                },
            },
            Grant {
                verb: "reach".to_owned(),
                scope: Scope::Anything,
            },
        ]);
        let ops = Real::gated(
            std::path::PathBuf::from("/w"),
            ledger,
            std::sync::Arc::new(crate::approve::AllowAll),
        )
        .isolating(true);
        let json = ops.jail().expect("a profile when isolation is on");
        assert!(
            json.contains("/w/build"),
            "the write grant is in the profile: {json}"
        );
        assert!(
            json.contains("\"reach\":true"),
            "the reach grant opens the network: {json}"
        );
    }

    /// The same, with the wall on: `magi.confine` is where that rule lives now.
    fn walled(name: &str) -> (Real, Scratch) {
        let dir = Scratch::new("magi-wall", name);
        (Real::confined(dir.to_path_buf()), dir)
    }

    #[test]
    fn a_write_then_a_read_round_trips() {
        let (ops, _dir) = rooted("roundtrip");
        ops.write(Path::new("a.txt"), "hello").expect("write");
        assert_eq!(ops.read(Path::new("a.txt")).expect("read"), "hello");
    }

    #[test]
    fn a_write_creates_parent_directories() {
        let (ops, dir) = rooted("parents");
        ops.write(Path::new("deep/nested/a.txt"), "x")
            .expect("write");
        assert!(dir.join("deep/nested/a.txt").exists());
    }

    #[test]
    fn a_confined_path_that_escapes_the_root_is_refused() {
        let (ops, _dir) = walled("escape");
        let error = ops
            .read(Path::new("../../etc/passwd"))
            .expect_err("must refuse");
        assert!(error.contains("outside"), "{error}");
    }

    #[test]
    fn an_escape_hidden_behind_a_descent_is_still_refused() {
        // `a/../../etc` has no leading `..` and still escapes, so the check happens after normalising.
        let (ops, _dir) = rooted("hidden");
        assert!(ops.read(Path::new("a/../../etc/passwd")).is_err());
    }

    #[test]
    fn a_confined_absolute_path_outside_the_root_is_refused() {
        let (ops, _dir) = walled("absolute");
        assert!(ops.read(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn a_command_that_fails_is_output_not_an_error() {
        // The model needs to see what it said; a non-zero exit is information, not a fault.
        let (ops, _dir) = rooted("failing");
        let result = ops.shell("echo out; echo err >&2; exit 3").expect("it ran");
        assert_eq!(result.code, Some(3));
        assert!(!result.ok());
        assert_eq!(result.stdout.trim(), "out");
        assert_eq!(result.stderr.trim(), "err");
    }

    #[test]
    fn a_command_runs_in_the_session_directory() {
        let (ops, _dir) = rooted("cwd");
        let result = ops.shell("pwd").expect("it ran");
        assert!(result.stdout.contains("magi-ops-"), "{}", result.stdout);
    }

    #[test]
    fn reading_a_missing_file_names_it() {
        let (ops, _dir) = rooted("missing");
        let error = ops.read(Path::new("nope.txt")).expect_err("must fail");
        assert!(error.contains("nope.txt"), "{error}");
    }
}

#[cfg(test)]
mod reach_tests {
    use super::*;
    use magi_model::scratch::Scratch;

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-reach", name)
    }

    #[test]
    fn a_path_outside_the_session_is_reachable() {
        // The refusal was not safety: a model told "outside this session's directory" uses `bash`.
        let session = scratch("session");
        let elsewhere = scratch("elsewhere");
        let file = elsewhere.join("hello.py");
        std::fs::write(&file, "print('a')\n").expect("write");

        let ops = Real::new(session.to_path_buf());
        assert_eq!(ops.read(&file).expect("read"), "print('a')\n");
        ops.write(&file, "print('b')\n").expect("write");
        assert_eq!(
            std::fs::read_to_string(&file).expect("read back"),
            "print('b')\n"
        );
    }

    #[test]
    fn a_relative_path_still_means_the_session() {
        // The root's real job: `src/main.rs` means what it means in the shell you started in.
        let session = scratch("relative");
        std::fs::create_dir_all(session.join("src")).expect("mkdir");
        std::fs::write(session.join("src/main.rs"), "fn main() {}\n").expect("write");
        let ops = Real::new(session.to_path_buf());
        assert_eq!(
            ops.read(Path::new("src/main.rs")).expect("read"),
            "fn main() {}\n"
        );
    }

    #[test]
    fn confined_ops_still_refuse_and_say_why() {
        let session = scratch("wall");
        let ops = Real::confined(session.to_path_buf());
        let outside = std::env::temp_dir().join("magi-not-here.txt");
        let why = ops.read(&outside).expect_err("refused");
        assert!(why.contains("magi.confine"), "it names the setting: {why}");
    }

    #[test]
    fn confinement_still_catches_a_path_that_climbs_out() {
        let session = scratch("climb");
        let ops = Real::confined(session.to_path_buf());
        assert!(ops.read(Path::new("a/../../etc/passwd")).is_err());
    }
}

#[path = "ops/gating.rs"]
#[cfg(test)]
mod gating;
