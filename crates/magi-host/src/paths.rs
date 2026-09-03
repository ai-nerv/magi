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
    base.join("magi").join("sessions")
}

/// A session identifier derived from the time it started, and what tells it from its neighbours.
///
/// Sortable as a string, which is what makes "the most recent session" a directory listing
/// rather than a database. The time alone is not enough: it has seconds of resolution, and two
/// sessions started in the same second named one journal between them and wrote into it
/// together. That is not a rare race now — starting a second `magi` beside the first is the
/// ordinary way to get two, and a person doing it does not pause a second first.
#[must_use]
pub fn session_id(now: u64, whose: &str) -> String {
    if whose.is_empty() {
        return format!("{now:020}");
    }
    format!("{now:020}-{whose}")
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
        assert!(sessions_dir().ends_with("magi/sessions"));
    }

    #[test]
    fn ids_sort_chronologically_as_strings() {
        let mut ids = [
            session_id(1_700_000_000, ""),
            session_id(9, ""),
            session_id(1_000, ""),
        ];
        ids.sort();
        assert_eq!(ids[0], session_id(9, ""), "{ids:?}");
        assert_eq!(ids[2], session_id(1_700_000_000, ""), "{ids:?}");
    }

    #[test]
    fn the_latest_journal_is_the_highest_id() {
        let dir = std::env::temp_dir().join(format!("magi-latest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for id in [session_id(1, ""), session_id(500, "")] {
            std::fs::write(dir.join(format!("{id}.jsonl")), "").expect("write");
        }
        std::fs::write(dir.join("notes.txt"), "").expect("write");

        let newest = latest(&dir).expect("a journal");
        assert!(newest.to_string_lossy().contains(&session_id(500, "")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_directory_has_no_latest() {
        let dir = std::env::temp_dir().join(format!("magi-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(latest(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    use magi_proto::SessionId;

    /// Two sessions in one directory and one in another, so "the latest" and "the latest here"
    /// are different journals.
    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-resume-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (id, cwd) in [(1_u64, "/work/a"), (2, "/work/a"), (3, "/work/b")] {
            let path = dir.join(format!("{}.jsonl", session_id(id, "")));
            magi_journal::Journal::open(&path, SessionId::new(session_id(id, "")), cwd, id)
                .expect("journal");
        }
        dir
    }

    #[test]
    fn resuming_finds_the_newest_session_for_this_directory() {
        let dir = fixture("scoped");
        let found = latest_for(&dir, "/work/a").expect("a journal");
        assert!(
            found.to_string_lossy().contains(&session_id(2, "")),
            "{found:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_newer_session_elsewhere_is_not_resumed_here() {
        // The bare `latest` would return the /work/b journal, which is the bug this avoids.
        let dir = fixture("elsewhere");
        let newest = latest(&dir).expect("a journal");
        assert!(newest.to_string_lossy().contains(&session_id(3, "")));
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

/// One session, as a picker needs to describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Its identity, which is also its file name.
    pub id: String,
    /// Where its journal is.
    pub path: PathBuf,
    /// The first thing the person said in it, or empty if they never said anything.
    ///
    /// A session's name is what it was for, and nobody titles one. The opening prompt is the
    /// closest thing to a title that exists without asking a model to invent one.
    pub title: String,
    /// How many entries it holds, so an abandoned session reads as abandoned.
    pub entries: usize,
}

/// Every session recorded for `cwd`, newest first.
///
/// Ids sort by time because they are timestamps, so "newest first" is a reversed sort rather
/// than a stat of every file. The whole journal is read to find the title and the count: they
/// are small, there are few of them, and a picker that lied about which session was which
/// would be worse than one that took a moment.
#[must_use]
pub fn summaries(dir: &std::path::Path, cwd: &str) -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut journals: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter(|p| recorded_cwd(p).as_deref() == Some(cwd))
        .collect();
    journals.sort();
    journals.reverse();
    journals.iter().filter_map(|path| summary(path)).collect()
}

/// Read one journal far enough to describe it.
fn summary(path: &std::path::Path) -> Option<Summary> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let text = std::fs::read_to_string(path).ok()?;
    let recovered = magi_journal::parse(&text).ok()?;
    let title = recovered
        .entries
        .iter()
        .find_map(|entry| match entry {
            magi_proto::Entry::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    Some(Summary {
        id,
        path: path.to_owned(),
        // One line, however it was typed: a picker row is one line and a prompt often is not.
        title: title.split_whitespace().collect::<Vec<_>>().join(" "),
        entries: recovered.entries.len(),
    })
}

/// The journal for one session id, if it is in `dir`.
#[must_use]
pub fn journal_for(dir: &std::path::Path, id: &str) -> Option<PathBuf> {
    // Built rather than searched, and then checked: an id is a file stem, so a caller that made
    // one up would otherwise be asking this to open whatever the name resolved to.
    let path = dir.join(format!("{id}.jsonl"));
    (path.parent() == Some(dir) && path.is_file()).then_some(path)
}

/// Every session balthasar holds for this project, newest first.
///
/// Asked rather than listed off disk. Once balthasar is the store, a directory of journals is
/// either absent or stale, and a picker built from stale files offers sessions that cannot be
/// resumed.
///
/// `None` when balthasar is not reachable, which is the caller's cue to fall back to the files
/// while a journal is still being kept.
#[must_use]
pub fn recorded() -> Option<Vec<Summary>> {
    let mut family = magi_ipc::family::blocking::Family::find().ok()?;
    let rows = family.call("sessions", Vec::new()).ok()?;
    Some(
        rows.iter()
            .flat_map(|value| match value.as_array() {
                Some(list) => list.clone(),
                None => vec![value.clone()],
            })
            .filter_map(|row| summary_of(&row))
            .collect(),
    )
}

/// One of balthasar's session rows, as a picker needs it.
///
/// `path` is empty: there is no file, and a caller that needs one is on the old path.
fn summary_of(row: &serde_json::Value) -> Option<Summary> {
    let id = row
        .get("id")
        .and_then(serde_json::Value::as_str)?
        .to_owned();
    let title = row
        .get("title")
        .or_else(|| row.get("name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Some(Summary {
        id,
        path: PathBuf::new(),
        title: title.split_whitespace().collect::<Vec<_>>().join(" "),
        entries: row
            .get("turns")
            .or_else(|| row.get("entries"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize,
    })
}
