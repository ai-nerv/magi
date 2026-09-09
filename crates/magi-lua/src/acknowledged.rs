//! What is installed, what it was when you said yes, and what changed since.
//!
//! A manifest names every installed package file and the digest it had when somebody acknowledged
//! it; a file whose digest does not match does not run. Only for what somebody else wrote: a file
//! in your own `plugin/` directory is one you put there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub path: PathBuf,
    /// Whether it was ever acknowledged, as against acknowledged and then changed.
    pub known: bool,
}

impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = if self.known {
            "has changed since it was acknowledged"
        } else {
            "has never been acknowledged"
        };
        write!(f, "{} {what}", self.path.display())
    }
}

#[must_use]
pub fn manifest_in(config: &Path) -> PathBuf {
    config.join("installed.json")
}

/// The digest of a file's contents, as the manifest records it. Of the bytes, not of the path: a
/// package moved to another directory is the same code.
#[must_use]
pub fn digest(source: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(source.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// What the manifest says, or an empty one. A manifest that will not parse reads as empty, which
/// holds everything back rather than letting everything through.
#[must_use]
pub fn recorded(manifest: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[must_use]
pub fn cleared(known: &BTreeMap<String, String>, path: &Path, source: &str) -> bool {
    known
        .get(&path.display().to_string())
        .is_some_and(|held| *held == digest(source))
}

#[must_use]
pub fn seen(known: &BTreeMap<String, String>, path: &Path) -> bool {
    known.contains_key(&path.display().to_string())
}

/// Write down what these files hold now. Replaces the manifest rather than merging into it, so
/// acknowledging after removing a package forgets it.
///
/// # Errors
/// When the manifest cannot be written.
pub fn acknowledge(manifest: &Path, files: &[(PathBuf, String)]) -> Result<usize, String> {
    let held: BTreeMap<String, String> = files
        .iter()
        .map(|(path, source)| (path.display().to_string(), digest(source)))
        .collect();
    let body = serde_json::to_string_pretty(&held)
        .map_err(|why| format!("the manifest cannot be written: {why}"))?;
    if let Some(parent) = manifest.parent() {
        std::fs::create_dir_all(parent).map_err(|why| format!("{}: {why}", parent.display()))?;
    }
    std::fs::write(manifest, body + "\n")
        .map_err(|why| format!("{}: {why}", manifest.display()))?;
    Ok(held.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn a_file_that_was_never_acknowledged_is_not_cleared() {
        let known = BTreeMap::new();
        assert!(!cleared(&known, Path::new("/p/a.lua"), "-- anything"));
        assert!(!seen(&known, Path::new("/p/a.lua")));
    }

    #[test]
    fn a_file_that_changed_after_being_acknowledged_is_not_cleared_and_is_known() {
        // The two states are different answers: "you have not looked at this" and "you looked at
        // it and it is not what you looked at".
        let dir = Scratch::new("magi-ack", "changed");
        let manifest = manifest_in(&dir);
        let path = dir.join("p.lua");
        acknowledge(&manifest, &[(path.clone(), "-- one".to_owned())]).expect("write");

        let known = recorded(&manifest);
        assert!(cleared(&known, &path, "-- one"), "unchanged runs");
        assert!(!cleared(&known, &path, "-- two"), "changed does not");
        assert!(seen(&known, &path), "and it says which of the two it is");
    }

    #[test]
    fn acknowledging_replaces_rather_than_accumulates() {
        // A manifest that only grows keeps a digest for a package nobody has any more.
        let dir = Scratch::new("magi-ack", "replace");
        let manifest = manifest_in(&dir);
        acknowledge(&manifest, &[(dir.join("a.lua"), "a".to_owned())]).expect("write");
        acknowledge(&manifest, &[(dir.join("b.lua"), "b".to_owned())]).expect("write");

        let known = recorded(&manifest);
        assert_eq!(known.len(), 1, "{known:?}");
        assert!(seen(&known, &dir.join("b.lua")));
        assert!(
            !seen(&known, &dir.join("a.lua")),
            "the old one is forgotten"
        );
    }

    #[test]
    fn a_manifest_that_will_not_parse_holds_everything_back() {
        // Reading a broken manifest as "everything is fine" would make a corrupt file the bypass.
        let dir = Scratch::new("magi-ack", "broken");
        let manifest = manifest_in(&dir);
        std::fs::write(&manifest, "{ this is not json").expect("write");
        let known = recorded(&manifest);
        assert!(known.is_empty());
        assert!(!cleared(&known, &dir.join("p.lua"), "-- anything"));
    }

    #[test]
    fn the_digest_is_of_the_contents_and_not_of_the_path() {
        // A package moved between directories is the same code; a file swapped under the same name
        // is not.
        assert_eq!(digest("-- same"), digest("-- same"));
        assert_ne!(digest("-- same"), digest("-- different"));
    }
}
