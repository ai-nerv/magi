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

/// The newest journal in `dir` that was recorded for `cwd`.
///
/// Journals are flat and global, so "the last session" on its own means the last session
/// anywhere -- which is the wrong one to resume as soon as two projects are open. The meta
/// line is the first line, so this reads one line per journal rather than parsing any of them.
#[must_use]
pub fn latest_for(dir: &std::path::Path, cwd: &str) -> Option<PathBuf> {
    let mut journals: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter(|p| recorded_cwd(p).as_deref() == Some(cwd))
        .collect();
    journals.sort();
    journals.pop()
}

/// The directory a journal says it was started in.
fn recorded_cwd(path: &std::path::Path) -> Option<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file).read_line(&mut first).ok()?;
    let meta: serde_json::Value = serde_json::from_str(&first).ok()?;
    meta.get("cwd")?.as_str().map(str::to_owned)
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

#[cfg(test)]
mod resume_tests {
    use super::*;
    use axum_proto::SessionId;

    /// Two sessions in one directory and one in another, so "the latest" and "the latest here"
    /// are different journals.
    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axum-resume-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (id, cwd) in [(1_u64, "/work/a"), (2, "/work/a"), (3, "/work/b")] {
            let path = dir.join(format!("{}.jsonl", session_id(id)));
            axum_journal::Journal::open(&path, SessionId::new(session_id(id)), cwd, id)
                .expect("journal");
        }
        dir
    }

    #[test]
    fn resuming_finds_the_newest_session_for_this_directory() {
        let dir = fixture("scoped");
        let found = latest_for(&dir, "/work/a").expect("a journal");
        assert!(
            found.to_string_lossy().contains(&session_id(2)),
            "{found:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_session_elsewhere_is_not_resumed_here() {
        // The bare `latest` would return the /work/b journal, which is the bug this avoids.
        let dir = fixture("elsewhere");
        let newest = latest(&dir).expect("a journal");
        assert!(newest.to_string_lossy().contains(&session_id(3)));
        let found = latest_for(&dir, "/work/a").expect("a journal");
        assert_ne!(found, newest);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_with_no_history_has_nothing_to_resume() {
        let dir = fixture("fresh");
        assert!(latest_for(&dir, "/work/never-seen").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
