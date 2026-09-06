//! Editing the prompt in `$EDITOR`.
//!
//! A coding prompt is a paragraph, and past a few lines the right tool is the user's own
//! editor. The terminal must be released first: a full-screen editor and a raw-mode TUI
//! cannot share a tty.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Suffix for the scratch file, so the editor picks a syntax.
const SUFFIX: &str = "magi-prompt.md";

/// Run one editor command over `text` and return what was saved.
///
/// `Ok(None)` means the prompt should be left exactly as it was: the editor exited non-zero
/// and the edit was abandoned. The command is a parameter rather than read from the
/// environment here so the round trip stays testable without `set_var`, which
/// `deny(unsafe_code)` rules out anyway.
pub fn edit_with(editor: &str, text: &str) -> Result<Option<String>> {
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        return Ok(None);
    };

    let path = scratch_path();
    let mut file =
        std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    drop(file);

    let status = Command::new(program)
        .args(parts)
        .arg(&path)
        .status()
        .with_context(|| format!("running {program}"));

    let result = match status {
        Ok(status) if status.success() => std::fs::read_to_string(&path)
            .ok()
            // A trailing newline is how every editor saves a file, never part of the prompt.
            .map(|t| t.trim_end_matches('\n').to_owned()),
        Ok(_) => None,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };

    let _ = std::fs::remove_file(&path);
    Ok(result)
}

/// The configured editor, preferring `$VISUAL` as convention requires.
#[must_use]
pub fn editor_command() -> Option<String> {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.trim().is_empty())
}

/// A scratch path unique to this call.
///
/// The counter matters: keying only on the process id makes two concurrent edits share a
/// file, and the second one silently wins.
fn scratch_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{}-{n}-{SUFFIX}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    #[test]
    fn an_editor_that_exits_clean_hands_back_the_file() {
        assert_eq!(
            edit_with("true", "original").expect("edit runs").as_deref(),
            Some("original")
        );
    }

    #[test]
    fn an_editor_that_rewrites_the_file_replaces_the_prompt() {
        let edited = edit_with("sed -i s/before/after/", "before").expect("edit runs");
        assert_eq!(edited.as_deref(), Some("after"));
    }

    #[test]
    fn a_failing_editor_abandons_the_edit() {
        assert_eq!(edit_with("false", "original").expect("edit runs"), None);
    }

    #[test]
    fn an_empty_editor_command_is_a_no_op() {
        assert_eq!(edit_with("   ", "original").expect("edit runs"), None);
    }

    #[test]
    fn the_scratch_file_does_not_survive_the_edit() {
        // The path the *edit* used, not a path nothing ever created. `scratch_path` takes a new
        // counter every call, so asking it for one and then asserting *that* one is absent said
        // nothing: it had never existed. The editor here records the path it was handed.
        // `cp -t <dir>` keeps the name it was given, so what lands in `dir` names the path the
        // edit actually used. A shell script written here and run immediately would have been
        // simpler and races: another test's `fork` inherits the write handle and `execve`
        // answers ETXTBSY.
        let dir = Scratch::new("magi-editor", "removed");
        let editor = format!("cp -t {}", dir.display());

        edit_with(&editor, "text").expect("edit runs");
        let copied = std::fs::read_dir(&*dir)
            .expect("read")
            .next()
            .expect("the editor was handed a path")
            .expect("entry")
            .file_name();
        let used = std::env::temp_dir().join(copied);
        assert!(!used.exists(), "{}", used.display());
    }

    #[test]
    fn a_missing_editor_binary_is_an_error_not_a_silent_no_op() {
        assert!(edit_with("magi-no-such-editor-binary", "text").is_err());
    }

    #[test]
    fn a_trailing_newline_from_the_editor_is_stripped() {
        // The command is split on whitespace, so the `sh -c '…'` this used to pass arrived as
        // five words, `sh` failed to parse the first of them, the edit was abandoned, and the
        // assertion held over `None` however this function behaved. `sed -i $a\\` appends to the
        // last line, which on a file with no terminator is exactly how an editor adds one.
        let edited = edit_with("sed -i $a\\", "line").expect("edit runs");
        assert_eq!(
            edited.as_deref(),
            Some("line"),
            "the prompt never keeps the editor's terminator"
        );
    }
}
