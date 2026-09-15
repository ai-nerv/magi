//! The prompts you have typed before: one file for every project, not one per project, appended
//! rather than rewritten until it grows past the cap. One prompt per line, in the shape every
//! shell's `history` has.

use std::io::Write;
use std::path::PathBuf;

/// How many prompts are kept. Trimmed on write rather than on read, so a file that grew before this
/// existed is brought back into line by the next prompt.
const KEEP: usize = 1000;

/// Where the history lives.
#[must_use]
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("magi").join("history")
}

/// Every prompt that was kept, oldest first. A missing or unreadable file is an empty history.
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

/// Add a prompt to the history, unless there is nothing to add. A repeat of the line already at the
/// end is dropped; a prompt spanning several lines is kept as one entry, with the newlines escaped.
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
    if let Some(parent) = path.parent()
        && let Err(why) = std::fs::create_dir_all(parent)
    {
        magi_model::noted!("history: {} could not be made: {why}", parent.display());
        return;
    }
    // Over the cap the file is rewritten, which is the one time anything is dropped; under it,
    // appended.
    if kept.len() > KEEP {
        let trimmed = kept.split_off(kept.len() - KEEP);
        if let Err(why) = std::fs::write(&path, trimmed.join("\n") + "\n") {
            magi_model::noted!("history: {} could not be rewritten: {why}", path.display());
        }
        return;
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            if let Err(why) = writeln!(file, "{prompt}") {
                magi_model::noted!("history: a prompt could not be appended: {why}");
            }
        }
        Err(why) => magi_model::noted!("history: {} could not be opened: {why}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_sits_beside_the_sessions() {
        let path = path();
        assert!(path.ends_with("magi/history"), "{}", path.display());
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
        let mut kept = vec!["one".to_owned(), "two".to_owned()];
        let repeat = "two";
        let dropped = kept.last().map(String::as_str) == Some(repeat);
        assert!(dropped);
        kept.push("three".to_owned());
        assert_eq!(kept.last().map(String::as_str), Some("three"));
    }

    #[test]
    fn a_file_that_grew_past_the_cap_is_brought_back_into_line() {
        let mut kept: Vec<String> = (0..KEEP + 5).map(|n| n.to_string()).collect();
        assert!(kept.len() > KEEP);
        let trimmed = kept.split_off(kept.len() - KEEP);
        assert_eq!(trimmed.len(), KEEP);
        assert_eq!(trimmed[0], "5", "the oldest went, not the newest");
    }

    #[test]
    fn blank_lines_in_the_file_are_not_prompts() {
        // A blank entry is one press of the up arrow that does nothing.
        let text = "one\n\ntwo\n\n";
        let read: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(read, vec!["one", "two"]);
    }
}
