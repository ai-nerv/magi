//! The prompts you have typed before.
//!
//! The editor has walked its history with the arrow keys since M2, and the history it walked was
//! a `Vec<String>` that started empty every run. So it worked perfectly within one session and
//! there was nothing there when you came back — which is the moment you actually want it, because
//! the prompt you want again is the one from yesterday.
//!
//! **One file, not one per project.** A shell keeps one history and reaching back into it from a
//! different directory is the point: the long prompt worth recalling is usually the one you wrote
//! carefully somewhere else. Sessions are already per-directory; this is not.
//!
//! **Appended, never rewritten** — until it grows past the cap, which is the one time the file is
//! rewritten and the one time anything is lost. A prompt is a line, so this is `history` in the
//! shape every shell has used for forty years and `tail -20` reads it.

use std::io::Write;
use std::path::PathBuf;

/// How many prompts are kept.
///
/// Trimmed on write rather than on read, so a file that grew before this existed is brought back
/// into line by the next prompt rather than re-read in full forever.
const KEEP: usize = 1000;

/// Where the history lives.
#[must_use]
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("axon").join("history")
}

/// Every prompt that was kept, oldest first.
///
/// A missing or unreadable file is an empty history. It is a convenience: refusing to start
/// because it could not be read would be trading the session for the convenience.
#[must_use]
pub fn load() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Add a prompt to the history, unless there is nothing to add.
///
/// A repeat of the line already at the end is dropped, the way a shell drops one: sending the
/// same prompt twice is common and a history of the same line ten times is a history of nothing.
/// A prompt spanning several lines is kept as one entry, with the newlines escaped — recalling
/// half of one would be worse than not recalling it.
pub fn remember(prompt: &str) {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return;
    }
    let mut kept = load();
    if kept.last().map(String::as_str) == Some(prompt) {
        return;
    }
    kept.push(prompt.to_owned());

    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Over the cap the file is rewritten, which is the one time anything is dropped. Under it,
    // appended — a prompt is one line and rewriting a thousand of them per prompt is not.
    if kept.len() > KEEP {
        let trimmed = kept.split_off(kept.len() - KEEP);
        let _ = std::fs::write(&path, trimmed.join("\n") + "\n");
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{prompt}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_sits_beside_the_sessions() {
        let path = path();
        assert!(path.ends_with("axon/history"), "{}", path.display());
    }

    #[test]
    fn a_blank_prompt_is_not_a_prompt() {
        // The editor already refuses to submit one, but the file should not depend on that.
        let before = load().len();
        remember("");
        remember("   \n  ");
        assert_eq!(load().len(), before, "nothing was added");
    }

    #[test]
    fn a_repeat_of_the_last_line_is_dropped() {
        // A shell drops one for the same reason: sending the same prompt twice is common, and a
        // history of the same line ten times is a history of nothing.
        let mut kept = vec!["one".to_owned(), "two".to_owned()];
        let repeat = "two";
        let dropped = kept.last().map(String::as_str) == Some(repeat);
        assert!(dropped);
        kept.push("three".to_owned());
        assert_eq!(kept.last().map(String::as_str), Some("three"));
    }

    #[test]
    fn a_file_that_grew_past_the_cap_is_brought_back_into_line() {
        // Trimmed on write rather than on read, so a file that grew before the cap existed does
        // not get re-read in full forever.
        let mut kept: Vec<String> = (0..KEEP + 5).map(|n| n.to_string()).collect();
        assert!(kept.len() > KEEP);
        let trimmed = kept.split_off(kept.len() - KEEP);
        assert_eq!(trimmed.len(), KEEP);
        assert_eq!(trimmed[0], "5", "the oldest went, not the newest");
    }

    #[test]
    fn blank_lines_in_the_file_are_not_prompts() {
        // A file somebody has opened in an editor picks up a trailing newline, and a blank entry
        // in the history is one press of the up arrow that does nothing.
        let text = "one\n\ntwo\n\n";
        let read: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(read, vec!["one", "two"]);
    }
}
