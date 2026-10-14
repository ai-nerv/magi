//! Where a program's sockets, notes and secrets live: `$XDG_RUNTIME_DIR` when it is safe to use,
//! else `<tmp>/<program>-<uid>`, and every directory made there private to this user.

use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// This program's runtime directory, created when it is missing and refused when it is not this
/// user's alone.
///
/// # Errors
/// When the directory cannot be made, or is not a private directory of this user.
pub fn runtime_dir(program: &str) -> io::Result<PathBuf> {
    private_dir(&runtime_base(program))
}

/// The same path, without creating or checking it: for naming a socket a client will dial.
#[must_use]
pub fn runtime_base(program: &str) -> PathBuf {
    match trusted_xdg() {
        Some(runtime) => runtime.join(program),
        None => std::env::temp_dir().join(format!("{program}-{}", uid())),
    }
}

/// `$XDG_RUNTIME_DIR`, when it is set, absolute, a directory, this user's and nobody else's.
#[must_use]
pub fn trusted_xdg() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?);
    let held = std::fs::symlink_metadata(&runtime).ok()?;
    (runtime.is_absolute() && private(&held)).then_some(runtime)
}

/// Make `path` a directory only this user may enter, or prove it already is one.
///
/// # Errors
/// When it cannot be made, or what is there is a link, another user's, or open to others.
pub fn private_dir(path: &Path) -> io::Result<PathBuf> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let held = std::fs::symlink_metadata(path)?;
    if !private(&held) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a directory private to this user: refused",
                path.display()
            ),
        ));
    }
    Ok(path.to_path_buf())
}

/// A real directory, owned by this uid, with no group or other permissions.
fn private(held: &std::fs::Metadata) -> bool {
    held.file_type().is_dir() && held.uid() == uid() && held.mode() & 0o077 == 0
}

fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Whether an `accept` that failed is a moment rather than an ending: out of descriptors, buffers
/// or memory, or a peer that left first. The listener keeps listening after a pause.
#[must_use]
pub fn transient(why: &io::Error) -> bool {
    use rustix::io::Errno;
    let passing = [
        Errno::MFILE,
        Errno::NFILE,
        Errno::NOBUFS,
        Errno::NOMEM,
        Errno::CONNABORTED,
        Errno::INTR,
        Errno::AGAIN,
    ];
    why.raw_os_error()
        .is_some_and(|code| passing.iter().any(|errno| errno.raw_os_error() == code))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_directory_made_here_is_private_and_one_open_to_others_is_refused() {
        let dir = magi_model::scratch::Scratch::new("magi-runtime", "private");
        let made = private_dir(&dir.join("mine")).expect("made");
        let mode = std::fs::metadata(&made).expect("stat").mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        assert!(
            private_dir(&made).is_ok(),
            "an existing private one is fine"
        );

        let open = dir.join("open");
        std::fs::create_dir(&open).expect("mkdir");
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let refused = private_dir(&open).expect_err("refused");
        assert!(
            refused.to_string().contains("open"),
            "names the path: {refused}"
        );
    }

    #[test]
    fn a_link_where_a_directory_belongs_is_refused() {
        let dir = magi_model::scratch::Scratch::new("magi-runtime", "link");
        let target = private_dir(&dir.join("real")).expect("made");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(private_dir(&link).is_err());
    }

    #[test]
    fn running_out_of_descriptors_is_a_moment_and_a_closed_listener_is_not() {
        let emfile = io::Error::from_raw_os_error(rustix::io::Errno::MFILE.raw_os_error());
        assert!(transient(&emfile));
        let gone = io::Error::from_raw_os_error(rustix::io::Errno::BADF.raw_os_error());
        assert!(!transient(&gone));
    }
}
