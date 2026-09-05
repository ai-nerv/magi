//! A temporary directory that removes itself.
//!
//! **Every test here used to clean up on its last line.** A `let _ = remove_dir_all(&dir);` after
//! the assertions runs when the test passes and does not run when it fails: `assert!` unwinds
//! straight past it. So the directories a *failing* test left behind stayed, and the delete-then-
//! create helpers only ever revisited their own name under their own pid — which never repeats.
//! The tree filled up quietly, across two renames of this project, and nothing anywhere said so.
//!
//! The fix is the one the language already offers: own the directory, and let `Drop` do it. A
//! guard runs on the unwind as well as on the return, which is the case that was leaking.
//!
//! In the model crate rather than the testkit because the testkit depends on half the workspace
//! and the crates that need this are among them; a leaf has no such problem. It is re-exported
//! from `magi_testkit` so a test still writes one name.

use std::path::{Path, PathBuf};

/// Distinguishes two scratches made in one process.
///
/// The pid alone is not enough. Two tests in one binary run on two threads, and a name is the
/// caller's to choose — so two that happened to choose the same one deleted each other's fixture
/// halfway through. A counter costs nothing and removes the question.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A directory under the temporary directory, removed when this is dropped.
///
/// Derefs to [`Path`], so a helper that used to hand back a `PathBuf` can hand back one of these
/// and every `dir.join(…)` at the call sites keeps compiling. What stops compiling is a caller
/// that wanted to *own* the path — those want [`Scratch::leak`] or `.to_path_buf()`, and the
/// compiler names each one.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A fresh directory, named after `prefix` and `name`.
    ///
    /// # Panics
    /// If the directory cannot be created, which is a broken machine rather than a failed test.
    #[must_use]
    pub fn new(prefix: &str, name: &str) -> Self {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{n}-{name}", std::process::id()));
        // Still removed first. A pid is reused eventually, and a run that was killed rather than
        // unwound leaves its directory behind for the next process that happens to match.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self { path }
    }

    /// Keep the directory, and stop owning it.
    ///
    /// For the handful of tests that inspect what was left behind after the thing that wrote it
    /// has gone. Whoever calls this owns the cleanup.
    #[must_use]
    pub fn leak(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

/// A path *inside* a scratch directory, where the directory is what is removed.
///
/// The shape a dozen helpers here already had: hand back the journal path, and clean up the
/// directory around it. Returning the [`Scratch`] instead would push the `join` out to every
/// caller for no gain, and holding only the file path would delete the directory the moment the
/// helper returned. Derefs to the file, so a caller writes `&temp("x")` as it always did.
#[derive(Debug)]
pub struct ScratchFile {
    /// Kept for its `Drop`, which is the entire point.
    _dir: Scratch,
    path: PathBuf,
}

impl Scratch {
    /// A named file inside a fresh scratch directory.
    #[must_use]
    pub fn file(prefix: &str, name: &str, file: &str) -> ScratchFile {
        let dir = Scratch::new(prefix, name);
        let path = dir.join(file);
        ScratchFile { _dir: dir, path }
    }
}

impl std::ops::Deref for ScratchFile {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchFile {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Ignored: the test has already said whether it passed, and a cleanup that panicked
        // during an unwind would abort the process and hide it.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::Scratch;

    #[test]
    fn a_scratch_removes_itself() {
        let path = {
            let dir = Scratch::new("magi-scratch", "gone");
            std::fs::write(dir.join("f"), "x").expect("write");
            dir.to_path_buf()
        };
        assert!(!path.exists(), "{}", path.display());
    }

    #[test]
    fn a_scratch_removes_itself_when_a_test_panics() {
        // The case the trailing `remove_dir_all` never covered, and the whole reason for this.
        let path = std::panic::catch_unwind(|| {
            let dir = Scratch::new("magi-scratch", "panicked");
            let path = dir.to_path_buf();
            std::fs::write(dir.join("f"), "x").expect("write");
            std::panic::panic_any(path);
        })
        .expect_err("the closure panics");
        let path = path.downcast::<std::path::PathBuf>().expect("the path");
        assert!(!path.exists(), "{}", path.display());
    }

    #[test]
    fn two_scratches_of_one_name_are_two_directories() {
        // Two tests in one binary may choose the same name, and a pid does not tell them apart.
        let a = Scratch::new("magi-scratch", "same");
        let b = Scratch::new("magi-scratch", "same");
        assert_ne!(a.to_path_buf(), b.to_path_buf());
        assert!(a.exists() && b.exists());
    }

    #[test]
    fn a_leaked_scratch_outlives_the_guard() {
        let path = Scratch::new("magi-scratch", "leaked").leak();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&path);
    }
}
