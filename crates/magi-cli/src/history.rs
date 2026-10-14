//! The prompts you have typed before: one file for every project, not one per project. One prompt
//! per line, in the shape every shell's `history` has, with a prompt's own newlines and
//! backslashes escaped so a prompt of several lines is still one line. Rewritten whole each time:
//! a private file, written under a lock and renamed into place.

use std::io::Write;
use std::path::{Path, PathBuf};

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
    load_from(&path())
}

fn load_from(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(unescaped)
        .collect()
}

/// Add a prompt to the history, unless there is nothing to add. A repeat of the line already at the
/// end is dropped; a prompt spanning several lines is kept as one entry, with the newlines escaped.
pub fn remember(prompt: &str) {
    if let Err(why) = remember_in(&path(), prompt) {
        magi_model::noted!("history: a prompt could not be kept: {why}");
    }
}

fn remember_in(path: &Path, prompt: &str) -> std::io::Result<()> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Ok(());
    }
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.with_extension("lock"))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;

    let mut kept = load_from(path);
    if kept.last().map(String::as_str) == Some(prompt) {
        return Ok(());
    }
    kept.push(prompt.to_owned());
    let kept = kept.split_off(kept.len().saturating_sub(KEEP));
    let mut text = String::new();
    for entry in &kept {
        text.push_str(&escaped(entry));
        text.push('\n');
    }

    let fresh = parent.join(format!(".history-{}", std::process::id()));
    let _ = std::fs::remove_file(&fresh);
    let mut file = magi_tools::family::private_file(&fresh)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&fresh, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&fresh);
    }
    written
}

/// One prompt as one line: a backslash, a newline and a carriage return each written as an escape.
fn escaped(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len());
    for c in prompt.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// The prompt a line of history holds.
fn unescaped(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    fn file(name: &str) -> (Scratch, PathBuf) {
        let dir = Scratch::new("magi-history", name);
        let path = dir.join("magi/history");
        (dir, path)
    }

    #[test]
    fn the_history_sits_beside_the_sessions() {
        let path = path();
        assert!(path.ends_with("magi/history"), "{}", path.display());
    }

    #[test]
    fn a_blank_prompt_is_not_a_prompt() {
        let (_dir, path) = file("blank");
        remember_in(&path, "").expect("kept");
        remember_in(&path, "   \n  ").expect("kept");
        assert!(load_from(&path).is_empty(), "nothing was added");
    }

    #[test]
    fn a_repeat_of_the_last_line_is_dropped() {
        let (_dir, path) = file("repeat");
        for prompt in ["one", "two", "two", "three"] {
            remember_in(&path, prompt).expect("kept");
        }
        assert_eq!(load_from(&path), ["one", "two", "three"]);
    }

    #[test]
    fn a_file_that_grew_past_the_cap_is_brought_back_into_line() {
        let (_dir, path) = file("cap");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        let grown: String = (0..KEEP + 5).map(|n| format!("{n}\n")).collect();
        std::fs::write(&path, grown).expect("write");
        remember_in(&path, "newest").expect("kept");
        let kept = load_from(&path);
        assert_eq!(kept.len(), KEEP);
        assert_eq!(kept[0], "6", "the oldest went, not the newest");
        assert_eq!(kept.last().map(String::as_str), Some("newest"));
    }

    #[test]
    fn blank_lines_in_the_file_are_not_prompts() {
        let (_dir, path) = file("blanks");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, "one\n\ntwo\n\n").expect("write");
        assert_eq!(load_from(&path), ["one", "two"]);
    }

    #[test]
    fn a_prompt_of_several_lines_is_one_entry_and_comes_back_whole() {
        let (_dir, path) = file("lines");
        let prompt = "first line\nsecond \\n literal\r\nthird";
        remember_in(&path, prompt).expect("kept");
        remember_in(&path, "after").expect("kept");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text.lines().count(), 2, "{text:?}");
        assert_eq!(load_from(&path), [prompt, "after"]);
    }

    #[test]
    fn the_history_is_private_and_replaced_whole() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = file("private");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, "older\n").expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        remember_in(&path, "newer").expect("kept");
        let mode = std::fs::metadata(&path)
            .expect("there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let left: Vec<_> = std::fs::read_dir(path.parent().expect("a parent"))
            .expect("listed")
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with(".history-"))
            .collect();
        assert!(left.is_empty(), "a temporary file was left: {left:?}");
    }
}
