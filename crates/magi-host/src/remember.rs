//! What a session should still know tomorrow: the model and effort chosen in the UI, keyed per
//! directory rather than globally. A cache beside the journals, not configuration — losing it costs
//! a preference, and `magi.model` is still where a chosen default is written down.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Chosen {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

#[must_use]
pub fn path() -> PathBuf {
    crate::paths::sessions_dir()
        .parent()
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("chosen.json")
}

/// What was chosen in `cwd` last time. An unreadable or unparseable file is "nothing was chosen".
#[must_use]
pub fn of(cwd: &str) -> Chosen {
    all().remove(cwd).unwrap_or_default()
}

/// Drop what `cwd` chose, leaving every other directory's choice alone. A directory with no row
/// falls back to the configuration, which is what `:reset model` is for.
pub fn forget(cwd: &str) {
    let _ = changed(&path(), |everything| everything.remove(cwd).is_some());
}

/// Remember `chosen` for `cwd`, keeping what every other directory chose. Read-modify-write, so two
/// daemons in two directories do not forget each other.
pub fn keep(cwd: &str, chosen: &Chosen) {
    let _ = changed(&path(), |everything| {
        everything.insert(cwd.to_owned(), chosen.clone());
        true
    });
}

/// Every directory's choices.
fn all() -> BTreeMap<String, Chosen> {
    read(&path())
}

fn read(path: &Path) -> BTreeMap<String, Chosen> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Read, change and write back the file at `path` under an exclusive lock beside it. Written to a
/// temporary file, synced and renamed into place, so a reader sees the old record or the new one
/// and never half of either. Nothing is written when `change` says it changed nothing.
fn changed(
    path: &Path,
    change: impl FnOnce(&mut BTreeMap<String, Chosen>) -> bool,
) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(parent.join("chosen.lock"))?;
    magi_ipc::exclusive(&lock)?;
    let mut everything = read(path);
    if !change(&mut everything) {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&everything).map_err(std::io::Error::other)?;
    let staged = parent.join(format!(".chosen.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&staged)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&staged, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_sits_beside_the_journals_rather_than_in_the_config() {
        let path = path();
        assert!(path.ends_with("magi/chosen.json"), "{}", path.display());
        assert!(
            !path.to_string_lossy().contains(".config"),
            "{}",
            path.display()
        );
    }

    #[test]
    fn a_directory_that_chose_nothing_has_chosen_nothing() {
        assert_eq!(of("/no/such/directory/ever"), Chosen::default());
    }

    #[test]
    fn a_choice_is_kept_per_directory() {
        let mut everything: BTreeMap<String, Chosen> = BTreeMap::new();
        everything.insert(
            "/a".to_owned(),
            Chosen {
                model: Some("one".into()),
                thinking: None,
            },
        );
        everything.insert(
            "/b".to_owned(),
            Chosen {
                model: Some("two".into()),
                thinking: Some("high".into()),
            },
        );
        let text = serde_json::to_string(&everything).expect("write");
        let read: BTreeMap<String, Chosen> = serde_json::from_str(&text).expect("read");
        assert_eq!(read["/a"].model.as_deref(), Some("one"));
        assert_eq!(read["/b"].thinking.as_deref(), Some("high"));
        assert_eq!(read["/a"].thinking, None);
    }

    #[test]
    fn a_record_that_will_not_parse_is_nothing_chosen_rather_than_a_failure() {
        let broken: Result<BTreeMap<String, Chosen>, _> = serde_json::from_str("{ not json");
        assert!(broken.is_err());
        assert_eq!(broken.unwrap_or_default().len(), 0);
    }

    #[test]
    fn writers_at_once_lose_nothing_and_leave_a_private_whole_record() {
        use std::os::unix::fs::MetadataExt;
        let dir = magi_model::scratch::Scratch::new("magi-chosen", "many");
        let path = dir.join("magi").join("chosen.json");
        let writers: Vec<_> = (0..16)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    changed(&path, |everything| {
                        everything.insert(format!("/dir/{n}"), Chosen::default());
                        true
                    })
                    .expect("written");
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("writer");
        }
        assert_eq!(read(&path).len(), 16, "a writer's row was lost");
        assert_eq!(
            std::fs::metadata(&path).expect("stat").mode() & 0o777,
            0o600
        );
    }
}
