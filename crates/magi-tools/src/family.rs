//! Which sockets are the family's, and how a client checks that one is ours. FAMILY.md's *Where
//! sockets live* and *Frames*, as the client side reads them. The runtime-directory rule itself
//! is `magi_ipc`'s, and the pieces of it used here are re-exported.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

pub use magi_ipc::family::{hold_key, holds_key, seal, sealed};
pub use magi_ipc::{Wire, peer_is_us, private_dir, private_file, runtime_base, uid};

/// The most bytes one frame may carry, in either direction.
pub const FRAME_CAP: usize = magi_ipc::family::MAX_FRAME_BYTES;

/// The directory names the family binds its sockets under, one per program or role.
pub const PROGRAMS: &[&str] = &["magi", "casper", "melchior", "memory", "balthasar"];

/// Every directory a family program may bind in: `<runtime>/<program>` and
/// `<tmp>/<program>-<uid>`, canonicalised where they exist.
#[must_use]
pub fn socket_roots() -> Vec<PathBuf> {
    socket_roots_in(magi_ipc::trusted_xdg().as_deref(), &std::env::temp_dir())
}

/// The same, for a runtime directory already decided and a temporary directory named.
#[must_use]
pub fn socket_roots_in(runtime: Option<&Path>, tmp: &Path) -> Vec<PathBuf> {
    let uid = uid();
    let mut out = Vec::new();
    for program in PROGRAMS {
        if let Some(runtime) = runtime {
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

/// A directory for a test's sockets inside `<tmp>/magi-<uid>`, where a VM may dial them. Both it
/// and the family directory above it pass [`private_dir`]. Removed on drop, and the family
/// directory with it once nothing else is in it.
#[doc(hidden)]
pub struct FamilyScratch {
    path: PathBuf,
}

static NEXT_SCRATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl FamilyScratch {
    /// # Errors
    /// When either directory cannot be made, or is not private to this user.
    pub fn new(name: &str) -> std::io::Result<Self> {
        let n = NEXT_SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let family = private_dir(&std::env::temp_dir().join(format!("magi-{}", uid())))?;
        let path = private_dir(&family.join(format!("t{}-{n}-{name}", std::process::id())))?;
        Ok(Self { path })
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
    use std::os::unix::net::UnixListener;

    #[test]
    fn only_a_socket_under_a_family_directory_is_dialable() {
        let dir = Scratch::new("magi-fam", "dial");
        let root = dir.join("memory");
        std::fs::create_dir_all(&root).expect("mkdir");
        let roots = vec![root.canonicalize().expect("real")];
        let ours = root.join("api.sock");
        let _listener = UnixListener::bind(&ours).expect("bind");
        assert!(family_socket_under(&ours, &roots).is_some());

        let stranger = dir.join("bus");
        let _bus = UnixListener::bind(&stranger).expect("bind");
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
    fn a_family_scratch_is_private_and_its_sockets_are_dialable() {
        let scratch = FamilyScratch::new("rule").expect("made");
        assert!(private_dir(&scratch).is_ok());
        assert!(private_dir(scratch.parent().expect("the family directory")).is_ok());
        let socket = scratch.join("s.sock");
        let _held = UnixListener::bind(&socket).expect("bind");
        assert!(family_socket(&socket).is_some(), "{}", socket.display());
    }

    #[test]
    fn a_private_scratch_is_a_trusted_runtime_and_its_sockets_are_dialable() {
        let dir = Scratch::new("magi-fam", "runtime");
        let runtime = dir.private("run");
        assert_eq!(
            magi_ipc::trusted(Some(runtime.clone().into_os_string())),
            Some(runtime.clone())
        );
        let home = private_dir(&runtime.join("melchior")).expect("made");
        let socket = home.join("s.sock");
        let _held = UnixListener::bind(&socket).expect("bind");
        let roots = socket_roots_in(Some(&runtime), &dir);
        assert!(family_socket_under(&socket, &roots).is_some());
        let beside = runtime.join("bus");
        let _bus = UnixListener::bind(&beside).expect("bind");
        assert!(family_socket_under(&beside, &roots).is_none());
    }
}
