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
const SUFFIX: &str = "axon-prompt.md";

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
        let before = scratch_path();
        edit_with("true", "text").expect("edit runs");
        assert!(
            !before.exists(),
            "the path this call would have used is gone"
        );
    }

    #[test]
    fn a_missing_editor_binary_is_an_error_not_a_silent_no_op() {
        assert!(edit_with("axon-no-such-editor-binary", "text").is_err());
    }

    #[test]
    fn a_trailing_newline_from_the_editor_is_stripped() {
        let edited = edit_with("sh -c 'printf \"line\\n\" >> \"$0\"' ", "").expect("edit runs");
        assert!(
            edited.is_none_or(|t| !t.ends_with('\n')),
            "the prompt never keeps the editor's terminator"
        );
    }
}
