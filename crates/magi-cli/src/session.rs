//! What one magi calls itself. A *key* — the pid and the clock — names this session's own files and
//! depends on nothing being installed; a *name* like `magi/main/psi-omicron` is
//! [`melchior`](crate::melchior)'s to give, arrives later, and may never arrive at all. A session
//! with no melchior still has a journal.

/// What this session's own files are named after: the pid, which no other running process shares,
/// and the clock, which no later session in the same pid slot will draw.
#[must_use]
pub fn key() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    format!("{:x}{now:05x}", std::process::id())
}

/// What this project is called: the working directory's last component, or what a config said.
#[must_use]
pub fn project(named: Option<&str>) -> String {
    named
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "magi".to_owned())
        })
}

/// Where this session's own socket lives: under magi's runtime directory, not melchior's, because
/// the UI talks to its own session whether or not the agent layer is there.
#[must_use]
pub fn socket_for(project: &str, key: &str) -> std::path::PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    socket_in(&runtime.join("magi").join(safe(project)), key)
}

/// The same name, in a directory somebody already has, so the `.host` suffix lives in one place:
/// what `--resume` needs when it holds a socket directory and asks whether that session is up.
#[must_use]
pub fn socket_in(dir: &std::path::Path, key: &str) -> std::path::PathBuf {
    dir.join(format!("{key}.host"))
}

/// Flatten a name into one path segment: a directory can be called anything, including `..`.
fn safe(name: &str) -> String {
    let out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if out.is_empty() { "-".to_owned() } else { out }
}

/// A key is this session's own, and a name is somebody else's to give.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_keys_from_one_process_are_still_two_keys() {
        // Sessions in one pid are sequential, so only the clock separates them.
        assert_ne!(key(), key());
    }

    #[test]
    fn a_key_is_a_filename_and_nothing_else() {
        let key = key();
        assert!(!key.is_empty());
        assert!(
            key.chars().all(|c| c.is_ascii_alphanumeric()),
            "a key goes in a path: {key}"
        );
    }

    #[test]
    fn a_project_is_the_folder_rather_than_the_path() {
        assert!(!project(None).contains('/'), "{}", project(None));
        assert_eq!(project(Some("chosen")), "chosen");
        assert_eq!(project(Some("   ")), project(None));
    }

    #[test]
    fn a_project_name_cannot_climb_out_of_the_runtime_directory() {
        let at = socket_for("../../etc", "abc123");
        assert!(!at.to_string_lossy().contains(".."), "{at:?}");
    }

    #[test]
    fn two_sessions_in_one_project_do_not_share_a_socket() {
        // Named after the *directory*, a second magi found the first one answering and joined it.
        assert_ne!(socket_for("magi", &key()), socket_for("magi", &key()));
    }
}
