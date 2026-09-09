//! A temporary directory that removes itself on `Drop`, so a failing test leaks nothing.
//!
//! In the model crate, not the testkit, because the crates that need it are among the testkit's
//! own dependencies.

use std::path::{Path, PathBuf};

/// Distinguishes two scratches made in one process; the pid does not tell two threads apart.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A directory under the temporary directory, removed when this is dropped. Derefs to [`Path`];
/// a caller that wants to own the path wants [`Scratch::leak`].
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
    /// Whether to wait for the processes working in here. See [`Scratch::settling`].
    settles: bool,
}

impl Scratch {
    /// A fresh directory, named after `prefix` and `name`. Panics if it cannot be created.
    #[must_use]
    pub fn new(prefix: &str, name: &str) -> Self {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{n}-{name}", std::process::id()));
        // A pid is reused eventually, and a run that was killed leaves its directory behind.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self {
            path,
            settles: false,
        }
    }

    /// Wait for the processes working in here to leave before removing it: a session's balthasar
    /// notices its magi die by looking, so its sqlite is briefly still open and a directory removed
    /// in that moment comes straight back. Not the default; it costs a walk of `/proc` per drop.
    #[must_use]
    pub fn settling(mut self) -> Self {
        self.settles = true;
        self
    }

    /// Keep the directory, and stop owning it. Whoever calls this owns the cleanup.
    #[must_use]
    pub fn leak(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

/// Whether any process still has its working directory inside `dir`. Asked of `/proc`, not of the
/// sockets a session leaves behind: one killed with `SIGKILL` never unlinks its socket.
fn anybody_in(dir: &Path) -> bool {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        // Unreadable is another user's, and a pid that has since finished is gone.
        .any(|entry| {
            std::fs::read_link(entry.path().join("cwd")).is_ok_and(|at| at.starts_with(dir))
        })
}

/// A path inside a scratch directory, where the directory is what is removed.
#[derive(Debug)]
pub struct ScratchFile {
    _dir: Scratch,
    path: PathBuf,
}

impl Scratch {
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
        if self.settles {
            // Bounded, because a wait with no end turns a leak into a hang.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline && anybody_in(&self.path) {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        // Ignored: a cleanup that panicked during an unwind would abort the process.
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
