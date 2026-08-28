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
}

impl Real {
    /// Ops rooted at `root`, reaching anywhere.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            confined: false,
        }
    }

    /// Ops that refuse anything outside `root`.
    #[must_use]
    pub fn confined(root: PathBuf) -> Self {
        Self {
            root,
            confined: true,
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
