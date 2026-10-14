//! Editing the prompt in `$EDITOR`. The terminal must be released first: a full-screen editor and
//! a raw-mode TUI cannot share a tty.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Suffix for the scratch file, so the editor picks a syntax.
const SUFFIX: &str = "magi-prompt.md";

/// Run one editor command over `text` and return what was saved. `Ok(None)` means the editor
/// exited non-zero and the prompt should be left exactly as it was.
pub fn edit_with(editor: &str, text: &str) -> Result<Option<String>> {
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        return Ok(None);
    };

    let path = scratch_path()?;
    let mut file = magi_tools::family::private_file(&path)
        .with_context(|| format!("creating {}", path.display()))?;
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

/// A scratch path unique to this call, inside magi's own private run-time directory. The counter
/// matters: keying only on the process id makes two concurrent edits share a file.
fn scratch_path() -> Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = magi_tools::family::private_dir(&magi_tools::family::own_dir("magi"))?;
    Ok(dir.join(format!("{}-{n}-{SUFFIX}", std::process::id())))
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
        // The editor records the path it was handed, because `scratch_path` takes a new counter
        // every call and asking it for a fresh one would assert over a path that never existed.
        let dir = Scratch::new("magi-editor", "removed");
        let editor = format!("cp -t {}", dir.display());

        edit_with(&editor, "text").expect("edit runs");
        let copied = std::fs::read_dir(&*dir)
            .expect("read")
            .next()
            .expect("the editor was handed a path")
            .expect("entry")
            .file_name();
        let used = magi_tools::family::own_dir("magi").join(copied);
        assert!(!used.exists(), "{}", used.display());
    }

    #[test]
    fn the_scratch_file_is_private_in_a_private_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new("magi-editor", "private");
        let said = dir.join("said");
        let editor = dir.join("editor");
        std::fs::write(
            &editor,
            format!(
                "#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\n\
                 stat -c '%a' \"$1\" \"$(dirname \"$1\")\" > '{}'\n",
                said.display()
            ),
        )
        .expect("write");
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        // `ETXTBSY` while a fork on another thread still holds the descriptor it was written by.
        while let Err(why) = Command::new(&editor).arg("--probe").status() {
            assert_eq!(
                why.raw_os_error(),
                Some(26),
                "the editor will not run: {why}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        edit_with(&editor.display().to_string(), "secret").expect("edit runs");
        let modes = std::fs::read_to_string(&said).expect("the editor ran");
        assert_eq!(modes.split_whitespace().collect::<Vec<_>>(), ["600", "700"]);
    }

    #[test]
    fn a_missing_editor_binary_is_an_error_not_a_silent_no_op() {
        assert!(edit_with("magi-no-such-editor-binary", "text").is_err());
    }

    #[test]
    fn a_trailing_newline_from_the_editor_is_stripped() {
        // The command is split on whitespace, so a `sh -c '…'` here would arrive as five words.
        let edited = edit_with("sed -i $a\\", "line").expect("edit runs");
        assert_eq!(
            edited.as_deref(),
            Some("line"),
            "the prompt never keeps the editor's terminator"
        );
    }
}
