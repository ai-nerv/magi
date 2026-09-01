//! Path candidates for `@` completion.
//!
//! Gitignore-aware, because Pi's is: it shells out to `fd`, which honours ignore files, and a
//! completion list led by build output or vendored checkouts is worse than no completion.
//!
//! Walked here rather than by the `ignore` crate. That crate is the right answer for a grep
//! tool and the wrong one for this: it reaches `globset` and then `regex-automata`, and the
//! three of them together were **eight hundred kilobytes of the binary** — eleven per cent of
//! it — to rank filenames in a popup eight rows tall. What this needs of a gitignore is the
//! handful of pattern forms people actually write in one, and `ignoring::Rule` is that.

mod ignoring;

use ignoring::Ignores;
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
    // From the root, not from `base`: a `.gitignore` at the top of the tree is what says
    // `target/` is uninteresting, and starting the search inside `src/` would never read it.
    let mut ignores = Ignores::from(root);
    let mut out = Vec::new();
    walk(root, &base, 0, &mut ignores, &mut out);
    out.sort();
    out
}

/// Walk one directory, and its children while there is room and depth left.
fn walk(root: &Path, here: &Path, depth: usize, ignores: &mut Ignores, out: &mut Vec<String>) {
    if depth >= MAX_DEPTH || out.len() >= MAX_ENTRIES {
        return;
    }
    ignores.read(here);
    let Ok(entries) = std::fs::read_dir(here) else {
        return;
    };
    // Sorted, because `read_dir` is in whatever order the filesystem holds them and a
    // completion list that reshuffles between keystrokes is unusable. The final sort orders the
    // whole result; this one decides which entries survive `MAX_ENTRIES`, which has to be the
    // same set every time or the list flickers.
    let mut found: Vec<std::fs::DirEntry> = entries.flatten().collect();
    found.sort_by_key(std::fs::DirEntry::file_name);

    for entry in found {
        if out.len() >= MAX_ENTRIES {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Hidden entries are not offered, and `.git` least of all.
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let folder = path.is_dir();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let mut shown = relative.to_string_lossy().into_owned();
        if ignores.hides(&shown, folder) {
            continue;
        }
        if folder {
            shown.push('/');
        }
        out.push(shown);
        if folder {
            walk(root, &path, depth + 1, ignores, out);
        }
    }
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
    fn an_ignored_directory_is_not_walked_into() {
        // Not just hidden from the list: descending into `target/` on every keystroke is the
        // latency this whole walk is bounded to avoid.
        let dir = fixture("descent");
        let found = list(&dir, "");
        assert!(
            !found.iter().any(|p| p.contains("debris")),
            "it walked in anyway: {found:?}"
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

    #[test]
    fn the_walk_is_bounded_however_deep_the_tree_goes() {
        // A recursive walk with no floor is a stack overflow waiting for a symlinked loop or a
        // node_modules nobody ignored.
        let dir = std::env::temp_dir().join(format!("axon-paths-{}-deep", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut at = dir.clone();
        for level in 0..20 {
            at = at.join(format!("level{level}"));
        }
        std::fs::create_dir_all(&at).expect("mkdir deep");
        let found = list(&dir, "");
        assert!(
            found.iter().all(|p| p.matches('/').count() <= MAX_DEPTH),
            "it went deeper than {MAX_DEPTH}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
