//! Where the memory role's sockets are, under the role's own name and the one it had before.

use std::path::{Path, PathBuf};

/// The role whose sockets magi looks for — the *job*, not a program. See `ROLES.md`.
const ROLE: &str = "memory";

/// What that directory was called when it was named after the program that filled the role.
const ROLE_WAS: &str = "balthasar";

/// The directory the memory role binds its sockets in: `$XDG_RUNTIME_DIR/memory`, else a
/// uid-suffixed temp directory, with `$MAGI_MEMORY_INSTANCE` selecting one when several run.
#[must_use]
pub fn socket_dir() -> PathBuf {
    named_dir(ROLE)
}

/// Where it bound them under the old name. Still looked in, for one release: a memory layer that
/// has not been rebuilt binds here alone, and the new name would find nothing while it runs.
#[must_use]
pub fn legacy_socket_dir() -> PathBuf {
    named_dir(ROLE_WAS)
}

/// Both directories, the role's own first. Read both, write the new one.
#[must_use]
pub fn socket_dirs() -> [PathBuf; 2] {
    [socket_dir(), legacy_socket_dir()]
}

/// One socket directory, with `$MAGI_MEMORY_INSTANCE` — or the name it used to have — selecting
/// one when several are running.
fn named_dir(name: &str) -> PathBuf {
    let base = match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join(name),
        None => std::env::temp_dir().join(format!("{name}-{}", rustix::process::getuid().as_raw())),
    };
    match instance() {
        Some(instance) => base.join(instance),
        None => base,
    }
}

/// Which instance the environment asks for, under either name. The role's own wins.
fn instance() -> Option<String> {
    ["MAGI_MEMORY_INSTANCE", "MAGI_BALTHASAR_INSTANCE"]
        .into_iter()
        .find_map(|named| std::env::var(named).ok().filter(|v| !v.is_empty()))
}

/// Every socket worth trying, newest first. `$MAGI_API_SOCKET` alone when it is set: a program
/// balthasar started inherits it and means *that* session. With no directory named, both of the
/// role's are searched and merged by age, so one bound only under the old name is still found.
#[must_use]
pub fn candidates(dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(named) = std::env::var_os("MAGI_API_SOCKET").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(named)];
    }
    match dir {
        Some(dir) => listing(dir),
        None => merged(&socket_dirs()),
    }
}

/// Every `api@*.sock` across several directories, newest first, each instance once.
///
/// One memory layer binds its instance under both names, so the same daemon appears twice under
/// the same file name. Either reaches it; the newer of the two is kept and the other dropped, so
/// the list is one entry per instance rather than one per door.
fn merged(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> =
        dirs.iter().flat_map(|dir| dated(dir)).collect();
    found.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .filter(|(_, path)| {
            path.file_name()
                .is_some_and(|name| seen.insert(name.to_owned()))
        })
        .map(|(_, path)| path)
        .collect()
}

/// Every `api@*.sock` in one directory, newest first. The directory and nothing else — no
/// `$MAGI_API_SOCKET` and no default location, unlike [`candidates`].
#[must_use]
pub fn sockets_in(dir: &Path) -> Vec<PathBuf> {
    listing(dir)
}

/// Every `api@*.sock` in one directory, newest first.
#[must_use]
fn listing(dir: &Path) -> Vec<PathBuf> {
    let mut found = dated(dir);
    // Newest first, so the key is reversed rather than the ordering.
    found.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Every `api@*.sock` in one directory, each with when it was last written, in no order.
fn dated(dir: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("api@") && name.ends_with(".sock")
        })
        .map(|e| {
            let when = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (when, e.path())
        })
        .collect()
}

/// Where magi looks for the memory role, now that the role's name is what the wire says.
#[cfg(test)]
mod tests {
    use super::{Path, legacy_socket_dir, listing, merged, socket_dir, socket_dirs};
    use magi_model::scratch::Scratch;

    #[test]
    fn the_role_is_looked_for_under_its_own_name_first() {
        assert!(socket_dir().ends_with("memory"), "{:?}", socket_dir());
        assert!(
            legacy_socket_dir().ends_with("balthasar"),
            "{:?}",
            legacy_socket_dir()
        );
        assert_eq!(socket_dirs(), [socket_dir(), legacy_socket_dir()]);
    }

    #[test]
    fn a_memory_layer_bound_only_under_the_old_name_is_still_found() {
        // The whole point of the window: a daemon that has not been rebuilt binds there alone,
        // and a magi that looked only at the new name would convene a second one beside it.
        let new = Scratch::new("magi-looking-new", "only-old");
        let old = Scratch::new("magi-looking-old", "only-old");
        let bound = old.join("api@earlier.sock");
        let _listening = std::os::unix::net::UnixListener::bind(&bound).expect("bind");

        let found = merged(&[new.to_path_buf(), old.to_path_buf()]);
        assert_eq!(found, vec![bound]);
    }

    #[test]
    fn one_daemon_answering_under_both_names_is_one_candidate_not_two() {
        let new = Scratch::new("magi-looking-new", "both");
        let old = Scratch::new("magi-looking-old", "both");
        for dir in [&new, &old] {
            let _bound =
                std::os::unix::net::UnixListener::bind(dir.join("api@twice.sock")).expect("bind");
        }

        let found = merged(&[new.to_path_buf(), old.to_path_buf()]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ends_with("api@twice.sock"), "{found:?}");
    }

    #[test]
    fn what_is_not_a_socket_of_the_roles_is_not_a_candidate() {
        let new = Scratch::new("magi-looking-new", "bystanders");
        let old = Scratch::new("magi-looking-old", "bystanders");
        std::fs::write(new.join("balthasar.tool"), "{}").expect("write");
        std::fs::write(old.join("given.lua"), "x").expect("write");

        assert!(merged(&[new.to_path_buf(), old.to_path_buf()]).is_empty());
    }

    #[test]
    fn a_directory_that_is_not_there_is_not_an_error() {
        let new = Scratch::new("magi-looking-new", "absent");
        assert!(merged(&[new.join("never"), new.join("nor-this")]).is_empty());
    }

    #[test]
    fn a_directory_that_is_not_there_offers_nothing() {
        assert!(listing(Path::new("/nonexistent/magi-family")).is_empty());
    }

    #[test]
    fn only_api_sockets_are_offered_and_the_newest_comes_first() {
        // Named after this process: a fixed path under a shared directory is one collision away
        // from two test binaries deleting each other's fixture.
        let dir = magi_model::scratch::Scratch::new("magi-family-listing", "one");
        for name in ["api@old.sock", "api@new.sock", "notes.txt", "api@x.other"] {
            std::fs::write(dir.join(name), b"").expect("write");
        }
        // The gap is *set*, not hoped for: two files written back to back can land in the same
        // filesystem tick, and the sort is stable, so equal times leave arbitrary `read_dir` order.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(dir.join("api@old.sock"))
            .expect("open")
            .set_modified(old)
            .expect("set mtime");

        let found = listing(&dir);
        assert_eq!(found.len(), 2, "only api@*.sock: {found:?}");
        assert!(
            found[0].ends_with("api@new.sock"),
            "newest first: {found:?}"
        );
    }
}
