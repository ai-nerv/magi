//! Where the family's sockets live, and how a client checks that one is ours. FAMILY.md's
//! *Where sockets live* and *Frames*, as the client side reads them.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

/// The most bytes one frame may carry, in either direction.
pub const FRAME_CAP: usize = 16 * 1024 * 1024;

/// The directory names the family binds its sockets under, one per program or role.
pub const PROGRAMS: &[&str] = &["magi", "casper", "melchior", "memory", "balthasar"];

/// This process's uid.
#[must_use]
pub fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// `$XDG_RUNTIME_DIR` when it is safe: absolute, a directory, owned by this uid, and closed to
/// group and other. Empty is unset.
#[must_use]
pub fn runtime_dir() -> Option<PathBuf> {
    safe_runtime(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

fn safe_runtime(said: Option<PathBuf>) -> Option<PathBuf> {
    let dir = said.filter(|dir| dir.is_absolute())?;
    let meta = std::fs::metadata(&dir).ok()?;
    (meta.is_dir() && meta.uid() == uid() && meta.mode() & 0o077 == 0).then_some(dir)
}

/// Every directory a family program may bind in: `<runtime>/<program>` and
/// `<tmp>/<program>-<uid>`, canonicalised where they exist.
#[must_use]
pub fn socket_roots() -> Vec<PathBuf> {
    let uid = uid();
    let runtime = runtime_dir();
    let tmp = std::env::temp_dir();
    let mut out = Vec::new();
    for program in PROGRAMS {
        if let Some(runtime) = &runtime {
            out.push(runtime.join(program));
        }
        out.push(tmp.join(format!("{program}-{uid}")));
    }
    out.into_iter()
        .map(|root| root.canonicalize().unwrap_or(root))
        .collect()
}

/// The path to dial for `path` when it is one of the family's sockets: its parent canonicalised
/// under a [`socket_roots`] directory, and the entry itself a socket owned by this uid.
#[must_use]
pub fn family_socket(path: &Path) -> Option<PathBuf> {
    family_socket_under(path, &socket_roots())
}

/// The same, against roots a caller names.
#[must_use]
pub fn family_socket_under(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = path.parent()?.canonicalize().ok()?;
    if !roots.iter().any(|root| parent.starts_with(root)) {
        return None;
    }
    let real = parent.join(name);
    let meta = std::fs::symlink_metadata(&real).ok()?;
    (meta.file_type().is_socket() && meta.uid() == uid()).then_some(real)
}

/// Whether the process on the far end of `stream` runs as this uid.
#[must_use]
pub fn peer_is_us(stream: &std::os::unix::net::UnixStream) -> bool {
    rustix::net::sockopt::get_socket_peercred(stream).is_ok_and(|cred| cred.uid.as_raw() == uid())
}

/// A directory for a test's sockets inside `<tmp>/magi-<uid>`, where a VM may dial them. Removed
/// on drop, and the family directory with it once nothing else is in it.
#[doc(hidden)]
pub struct FamilyScratch {
    path: PathBuf,
}

static NEXT_SCRATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl FamilyScratch {
    /// # Panics
    /// When the directory cannot be made.
    #[must_use]
    pub fn new(name: &str) -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let n = NEXT_SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("magi-{}", uid()))
            .join(format!("t{}-{n}-{name}", std::process::id()));
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)
            .expect("a family scratch directory");
        Self { path }
    }
}

impl std::ops::Deref for FamilyScratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for FamilyScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn an_unsafe_runtime_directory_is_not_used() {
        let dir = Scratch::new("magi-fam", "runtime");
        std::fs::set_permissions(&*dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(
            safe_runtime(Some(dir.to_path_buf())),
            None,
            "group-readable"
        );
        std::fs::set_permissions(&*dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        assert_eq!(
            safe_runtime(Some(dir.to_path_buf())),
            Some(dir.to_path_buf())
        );
        assert_eq!(
            safe_runtime(Some(PathBuf::from(""))),
            None,
            "empty is unset"
        );
        assert_eq!(safe_runtime(Some(PathBuf::from("run"))), None, "relative");
        assert_eq!(safe_runtime(None), None);
    }

    #[test]
    fn only_a_socket_under_a_family_directory_is_dialable() {
        let dir = Scratch::new("magi-fam", "dial");
        let root = dir.join("memory");
        std::fs::create_dir_all(&root).expect("mkdir");
        let roots = vec![root.canonicalize().expect("real")];
        let ours = root.join("api.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&ours).expect("bind");
        assert!(family_socket_under(&ours, &roots).is_some());

        let stranger = dir.join("bus");
        let _bus = std::os::unix::net::UnixListener::bind(&stranger).expect("bind");
        assert!(
            family_socket_under(&stranger, &roots).is_none(),
            "beside the family directory"
        );
        let climbing = root.join("../bus");
        assert!(family_socket_under(&climbing, &roots).is_none(), "`..`");

        let file = root.join("plain");
        std::fs::write(&file, "x").expect("write");
        assert!(family_socket_under(&file, &roots).is_none(), "not a socket");

        let link = root.join("link.sock");
        std::os::unix::fs::symlink(&stranger, &link).expect("symlink");
        assert!(family_socket_under(&link, &roots).is_none(), "a symlink");
    }

    #[test]
    fn a_peer_in_this_process_is_this_uid() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().expect("pair");
        assert!(peer_is_us(&a));
    }
}
