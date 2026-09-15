//! Where a session's *name* comes from, and where its sockets go. History is balthasar's, not
//! this module's: [`recorded`] asks it what sessions exist rather than listing anything.

use std::path::PathBuf;

/// The directory a session's socket and preferences go under. Holds no transcripts.
#[must_use]
pub fn sessions_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("magi").join("sessions")
}

/// A session identifier derived from its start time, sortable as a string. The time alone is not
/// enough: it has seconds of resolution, and two sessions can start in the same second.
#[must_use]
pub fn session_id(now: u64, whose: &str) -> String {
    if whose.is_empty() {
        return format!("{now:020}");
    }
    format!("{now:020}-{whose}")
}

/// One session, as a picker needs to describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    /// The opening prompt, or empty if they never said anything.
    pub title: String,
    pub entries: usize,
}

/// Every session balthasar holds, newest first.
#[must_use]
pub fn recorded() -> Vec<Summary> {
    let Ok(mut family) = magi_ipc::family::blocking::Family::find() else {
        return Vec::new();
    };
    let Ok(rows) = family.call("sessions", Vec::new()) else {
        return Vec::new();
    };
    rows.iter()
        .flat_map(|value| match value.as_array() {
            Some(list) => list.clone(),
            None => vec![value.clone()],
        })
        .filter_map(|row| summary_of(&row))
        .collect()
}

/// One of balthasar's session rows, as a picker needs it.
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
        title: title.split_whitespace().collect::<Vec<_>>().join(" "),
        entries: row
            .get("turns")
            .or_else(|| row.get("entries"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize,
    })
}

#[cfg(test)]
#[path = "paths/naming.rs"]
mod naming;
