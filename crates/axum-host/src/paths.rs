//! Where sessions live on disk.

use std::path::PathBuf;

/// The directory holding every session journal.
///
/// Flat and global rather than per-project: a session records its own `cwd` in its meta line,
/// so listing by project is a filter rather than a directory layout, and a session that moves
/// between worktrees does not strand its history.
#[must_use]
pub fn sessions_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("axum").join("sessions")
}

/// A session identifier derived from the time it started.
///
/// Sortable as a string, which is what makes "the most recent session" a directory listing
/// rather than a database.
#[must_use]
pub fn session_id(now: u64) -> String {
    format!("{now:020}")
}

/// The newest session journal in `dir`, if there is one.
#[must_use]
pub fn latest(dir: &std::path::Path) -> Option<PathBuf> {
    let mut journals: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    journals.sort();
    journals.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_live_under_a_data_directory() {
        assert!(sessions_dir().ends_with("axum/sessions"));
    }

    #[test]
    fn ids_sort_chronologically_as_strings() {
        let mut ids = [session_id(1_700_000_000), session_id(9), session_id(1_000)];
        ids.sort();
        assert_eq!(ids[0], session_id(9), "{ids:?}");
        assert_eq!(ids[2], session_id(1_700_000_000), "{ids:?}");
    }

    #[test]
    fn the_latest_journal_is_the_highest_id() {
        let dir = std::env::temp_dir().join(format!("axum-latest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for id in [session_id(1), session_id(500)] {
            std::fs::write(dir.join(format!("{id}.jsonl")), "").expect("write");
        }
        std::fs::write(dir.join("notes.txt"), "").expect("write");

        let newest = latest(&dir).expect("a journal");
        assert!(newest.to_string_lossy().contains(&session_id(500)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_directory_has_no_latest() {
        let dir = std::env::temp_dir().join(format!("axum-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(latest(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
