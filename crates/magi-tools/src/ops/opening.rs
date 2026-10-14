//! Opening a path the gate has already resolved, so the open lands where the gate looked: beneath
//! the root when confined, and through no link at all otherwise. `openat2` resolves where the
//! kernel has it; without it (`ENOSYS`) the path is canonicalised and its leaf is not followed.

use rustix::fs::{Mode, OFlags, ResolveFlags};
use std::fs::File;
use std::io;
use std::path::Path;

/// Open `rel` beneath `root`. A `..` or a link that leaves the root is refused with `EXDEV`.
pub(super) fn beneath(root: &Path, rel: &Path, flags: OFlags, mode: Mode) -> io::Result<File> {
    let dir = rustix::fs::open(
        root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let how = ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS;
    match rustix::fs::openat2(&dir, rel, flags | OFlags::CLOEXEC, mode, how) {
        Err(rustix::io::Errno::NOSYS) => canonical_beneath(root, rel, flags, mode),
        opened => Ok(File::from(opened?)),
    }
}

/// [`beneath`] without `openat2`: canonicalised, checked against the root, leaf not followed.
fn canonical_beneath(root: &Path, rel: &Path, flags: OFlags, mode: Mode) -> io::Result<File> {
    let root = super::real_path(root);
    let real = super::real_path(&root.join(rel));
    if !real.starts_with(&root) {
        return Err(rustix::io::Errno::XDEV.into());
    }
    leaf(&real, flags, mode)
}

/// Open the canonical path `real` through no symbolic or magic link.
pub(super) fn unlinked(real: &Path, flags: OFlags, mode: Mode) -> io::Result<File> {
    let how = ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
    match rustix::fs::openat2(rustix::fs::CWD, real, flags | OFlags::CLOEXEC, mode, how) {
        Err(rustix::io::Errno::NOSYS) => leaf(real, flags, mode),
        opened => Ok(File::from(opened?)),
    }
}

/// The fallback open: `O_NOFOLLOW` on the leaf.
fn leaf(real: &Path, flags: OFlags, mode: Mode) -> io::Result<File> {
    let flags = flags | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    Ok(File::from(rustix::fs::open(real, flags, mode)?))
}

/// Whether `why` is the refusal of a path that resolves outside the root.
pub(super) fn escaped(why: &io::Error) -> bool {
    why.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;

    /// A root holding `sub/f`, an inner link to `sub`, and a link out to a sibling holding a secret.
    fn laid_out(name: &str) -> (Scratch, std::path::PathBuf) {
        let dir = Scratch::new("magi-beneath", name);
        let root = dir.join("root");
        std::fs::create_dir_all(root.join("sub")).expect("mkdir");
        std::fs::create_dir_all(dir.join("outside")).expect("mkdir");
        std::fs::write(root.join("sub/f"), "inside").expect("write");
        std::fs::write(dir.join("outside/secret"), "outside").expect("write");
        std::os::unix::fs::symlink("sub", root.join("inner")).expect("symlink");
        std::os::unix::fs::symlink(dir.join("outside"), root.join("out")).expect("symlink");
        (dir, root)
    }

    type Opener = fn(&Path, &Path, OFlags, Mode) -> io::Result<File>;

    fn read_with(open: Opener, root: &Path, rel: &str) -> io::Result<String> {
        let mut file = open(root, Path::new(rel), OFlags::RDONLY, Mode::empty())?;
        let mut text = String::new();
        io::Read::read_to_string(&mut file, &mut text)?;
        Ok(text)
    }

    #[test]
    fn a_symlinked_parent_or_a_climb_out_of_the_root_is_refused() {
        let (_dir, root) = laid_out("kernel");
        for open in [beneath as Opener, canonical_beneath] {
            assert_eq!(read_with(open, &root, "sub/f").expect("inside"), "inside");
            for escape in [
                "out/secret",
                "../outside/secret",
                "sub/../../outside/secret",
            ] {
                let why = read_with(open, &root, escape).expect_err(escape);
                assert!(escaped(&why), "{escape}: {why}");
            }
        }
    }

    #[test]
    fn a_link_that_stays_beneath_the_root_is_followed() {
        let (_dir, root) = laid_out("inner");
        for open in [beneath as Opener, canonical_beneath] {
            assert_eq!(read_with(open, &root, "inner/f").expect("inner"), "inside");
        }
    }

    #[test]
    fn an_unconfined_open_follows_no_link_put_in_after_the_gate_looked() {
        let (_dir, root) = laid_out("unlinked");
        let read = |path: &Path| unlinked(path, OFlags::RDONLY, Mode::empty());
        assert!(read(&root.join("sub/f")).is_ok());
        assert!(
            read(&root.join("inner/f")).is_err(),
            "through a parent link"
        );
        assert!(
            read(&root.join("out/secret")).is_err(),
            "through a link out"
        );
    }

    #[test]
    fn a_write_through_a_parent_that_leaves_the_root_creates_nothing() {
        let (dir, root) = laid_out("write");
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC;
        let mode = Mode::from_raw_mode(0o600);
        for open in [beneath as Opener, canonical_beneath] {
            assert!(open(&root, Path::new("sub/new"), flags, mode).is_ok());
            let why = open(&root, Path::new("out/planted"), flags, mode).expect_err("out");
            assert!(escaped(&why), "{why}");
            assert!(!dir.join("outside/planted").exists());
        }
    }
}
