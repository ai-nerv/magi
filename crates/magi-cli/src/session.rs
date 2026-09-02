//! What one magi calls itself, before anything else has an opinion.
//!
//! Two different names, and keeping them apart is the point.
//!
//! **A key** is what makes this session's files its own: the socket the UI talks to its own
//! session over, and the journal it writes. It has to exist before anything starts, it has to be
//! unique among the magi sessions running now, and it must not depend on another program being
//! installed. So it is the process id and the clock, and it is never shown to anybody.
//!
//! **A name** is what a person and a sibling call this session: `magi/main/psi-omicron`. That is
//! [`melchior`](crate::melchior)'s to give, because melchior holds the directory those names live in and can
//! look before it chooses. It arrives a moment after startup and it may never arrive at all.
//!
//! They were one thing for a while, and it made the harness depend on the layer for the ability
//! to open a file. A session with no melchior still has a journal.

/// What this session's own files are named after.
///
/// The pid, which no other running process shares, and the clock, which no *later* session in
/// the same pid slot will draw. Together they are unique among the sessions that could collide,
/// which is all this has to be — it is not an identifier anybody quotes.
#[must_use]
pub fn key() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    format!("{:x}{now:05x}", std::process::id())
}

/// What this project is called: the working directory's name, or what a config said.
///
/// The last component, not the path — `/home/you/work/magi` is `magi`, because that is what a
/// person calls it. A directory with no name falls back rather than leaving the half of a name
/// empty.
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

/// Where this session's own socket lives.
///
/// Under magi's runtime directory, not melchior's: this is the UI talking to its own session, and it
/// exists whether or not the agent layer does.
#[must_use]
pub fn socket_for(project: &str, key: &str) -> std::path::PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    socket_in(&runtime.join("magi").join(safe(project)), key)
}

/// The same name, in a directory somebody already has.
///
/// What `--resume` needs: it is holding this project's socket directory and asking whether the
/// session that wrote a journal is still up. Here rather than spelled out there, so the suffix
/// is in one place — two of them disagreeing would have every resume find nothing listening.
#[must_use]
pub fn socket_in(dir: &std::path::Path, key: &str) -> std::path::PathBuf {
    dir.join(format!("{key}.host"))
}

/// Flatten a name into one path segment.
///
/// A project is a directory's name and a directory can be called anything, including `..`.
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
        // Sessions in one pid are sequential, so the clock separates them — and it has to,
        // because two `magi -p` in a script share nothing else.
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
        // Otherwise `magi.project = ""` names a session after nothing.
        assert_eq!(project(Some("   ")), project(None));
    }

    #[test]
    fn a_project_name_cannot_climb_out_of_the_runtime_directory() {
        // A project is a directory's name, and a directory can be called `..`.
        let at = socket_for("../../etc", "abc123");
        assert!(!at.to_string_lossy().contains(".."), "{at:?}");
    }

    #[test]
    fn two_sessions_in_one_project_do_not_share_a_socket() {
        // The bug that started all of this: named after the *directory*, a second magi found
        // the first one already answering and joined it.
        assert_ne!(socket_for("magi", &key()), socket_for("magi", &key()));
    }
}
