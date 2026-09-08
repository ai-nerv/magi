//! Where a session's *name* comes from, and where its sockets go.
//!
//! **Not where its history lives.** That is balthasar's, and only balthasar's — this module once
//! held a directory of JSONL journals, a listing of them, a torn-tail recovery, and a "newest one
//! for this cwd" search. All of it is gone: two stores is one store and a copy that goes stale,
//! and a session resumed from the stale one resumes into something that half-happened.
//!
//! What is left is naming and placement. A session id is a sortable timestamp, the directory
//! under it holds sockets, and [`recorded`] asks balthasar what sessions exist rather than
//! listing anything.

use std::path::PathBuf;

/// The directory a session's socket and preferences go under.
///
/// Named `sessions` for what it once held. It holds no transcripts now — see the module docs —
/// but a session still needs a place on this machine to be reachable at, and
/// `magi_host::remember` keeps the model somebody last chose beside it.
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
/// Sortable as a string, so "the most recent session" is an ordering rather than a search. The
/// time alone is not enough: it has seconds of resolution, and two sessions started in the same
/// second took the same name. That is not a rare race — starting a second `magi` beside the first
/// is the ordinary way to get two, and a person doing it does not pause a second first.
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
    /// Its identity, as balthasar knows it.
    pub id: String,
    /// The first thing the person said in it, or empty if they never said anything.
    ///
    /// A session's name is what it was for, and nobody titles one. The opening prompt is the
    /// closest thing to a title that exists without asking a model to invent one.
    pub title: String,
    /// How many entries it holds, so an abandoned session reads as abandoned.
    pub entries: usize,
}

/// Every session balthasar holds, newest first.
///
/// **Asked, never listed.** balthasar is the store, so it is the only thing that knows what
/// exists. Empty when it has nothing — and, since magi will not start without it, never empty
/// *because nobody asked*.
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
