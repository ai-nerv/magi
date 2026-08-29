//! Path candidates for `@` completion.
//!
//! Gitignore-aware, because Pi's is: it shells out to `fd`, which honours ignore files, and a
//! completion list led by build output or vendored checkouts is worse than no completion.

use ignore::WalkBuilder;
use std::path::Path;

/// How deep to walk. The popup shows eight rows; a deep tree costs a keystroke's latency for
/// candidates nobody scrolls to.
const MAX_DEPTH: usize = 6;

/// How many entries to collect before giving up on the walk.
const MAX_ENTRIES: usize = 4000;

/// List paths under `root` matching the directory part of `query`.
///
/// The query's final segment is left to the fuzzy ranker; only its directory prefix narrows
/// the walk, so `src/ma` scans `src/` rather than the whole tree.
#[must_use]
pub fn list(root: &Path, query: &str) -> Vec<String> {
    let prefix = query.rfind('/').map_or("", |i| &query[..=i]);
    let base = root.join(prefix);
    if !base.is_dir() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let walk = WalkBuilder::new(&base)
        .max_depth(Some(MAX_DEPTH))
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .build();

    for entry in walk.flatten() {
        if out.len() >= MAX_ENTRIES {
            break;
        }
        // The walker yields its own root first; offering the directory the user already typed
        // is not a completion.
        if entry.path() == base {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let mut path = relative.to_string_lossy().into_owned();
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            path.push('/');
        }
        out.push(path);
    }

    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture tree of its own, because these tests run on parallel threads and a shared
    /// directory means one test deleting another's files mid-walk.
    fn fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("axon-paths-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
        std::fs::create_dir_all(dir.join("target")).expect("mkdir target");
        std::fs::write(dir.join(".gitignore"), "target/\nvendored/\n").expect("write ignore");
        std::fs::create_dir_all(dir.join("vendored")).expect("mkdir vendored");
        std::fs::write(dir.join("src/main.rs"), "").expect("write");
        std::fs::write(dir.join("src/lib.rs"), "").expect("write");
        std::fs::write(dir.join("Cargo.toml"), "").expect("write");
        std::fs::write(dir.join("target/debris"), "").expect("write");
        std::fs::write(dir.join("vendored/huge.rs"), "").expect("write");
        dir
    }

    #[test]
    fn files_and_directories_are_listed_relative_to_the_root() {
        let dir = fixture("listing");
        let found = list(&dir, "");
        assert!(found.contains(&"Cargo.toml".to_owned()), "{found:?}");
        assert!(found.contains(&"src/".to_owned()), "{found:?}");
        assert!(found.contains(&"src/main.rs".to_owned()), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gitignored_paths_are_not_offered() {
        let dir = fixture("ignored");
        let found = list(&dir, "");
        assert!(!found.iter().any(|p| p.starts_with("target")), "{found:?}");
        assert!(
            !found.iter().any(|p| p.starts_with("vendored")),
            "{found:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dotfiles_are_not_offered() {
        let dir = fixture("dotfiles");
        let found = list(&dir, "");
        assert!(!found.iter().any(|p| p.starts_with('.')), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_prefix_narrows_the_walk() {
        let dir = fixture("prefix");
        let found = list(&dir, "src/ma");
        assert!(!found.is_empty(), "the prefix directory is scanned");
        assert!(found.iter().all(|p| p.starts_with("src/")), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_yields_nothing_rather_than_failing() {
        let dir = fixture("missing");
        assert!(list(&dir, "nope/").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
