//! Secrets this family hands over in files: random bytes as hex, and the key a session's socket is
//! controlled with, in `<socket>.key` beside it.

use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// `bytes` random bytes from the kernel, as lowercase hex.
///
/// # Errors
/// When the kernel's random source cannot be read.
pub fn random_hex(bytes: usize) -> io::Result<String> {
    let mut raw = vec![0_u8; bytes];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// Where the key for the session listening on `socket` is kept.
#[must_use]
pub fn key_path(socket: &Path) -> PathBuf {
    let mut named = socket.as_os_str().to_owned();
    named.push(".key");
    PathBuf::from(named)
}

/// Write a fresh key for `socket`, readable by this user alone, replacing any left by a crash.
///
/// # Errors
/// When there is no randomness or the file cannot be written.
pub fn mint_key(socket: &Path) -> io::Result<String> {
    let key = random_hex(32)?;
    let path = key_path(socket);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    crate::private_file(&path)?.write_all(key.as_bytes())?;
    Ok(key)
}

/// The key for `socket`, when this user's file holds one — see [`read_private`].
#[must_use]
pub fn read_key(socket: &Path) -> Option<String> {
    let key = read_private(&key_path(socket), 128)?;
    (key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())).then_some(key)
}

/// At most `most` bytes of `path`, trimmed, when it is a regular file and not a link, owned by
/// this uid and readable by nobody else.
#[must_use]
pub fn read_private(path: &Path, most: u64) -> Option<String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .ok()?;
    let held = file.metadata().ok()?;
    if !held.is_file() || held.uid() != crate::uid() || held.mode() & 0o077 != 0 {
        return None;
    }
    let mut said = String::new();
    file.take(most).read_to_string(&mut said).ok()?;
    Some(said.trim().to_owned())
}

/// Remove the key for `socket`.
pub fn drop_key(socket: &Path) {
    let _ = std::fs::remove_file(key_path(socket));
}

/// Whether two keys are the same, taking as long whichever byte differs.
#[must_use]
pub fn same_key(given: &str, held: &str) -> bool {
    let (given, held) = (given.as_bytes(), held.as_bytes());
    given.len() == held.len()
        && given
            .iter()
            .zip(held)
            .fold(0_u8, |differ, (a, b)| differ | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_key_is_read_back_by_its_owner_and_nothing_else_reads_as_one() {
        let dir = magi_model::scratch::Scratch::new("magi-key", "one");
        let socket = dir.join("s.sock");
        let key = mint_key(&socket).expect("mint");
        assert_eq!(key.len(), 64);
        let mode = std::fs::metadata(key_path(&socket)).expect("stat").mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        assert_eq!(read_key(&socket).as_deref(), Some(key.as_str()));

        let again = mint_key(&socket).expect("mint again");
        assert_ne!(again, key, "a fresh key each time it binds");

        std::fs::set_permissions(
            key_path(&socket),
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        )
        .expect("chmod");
        assert_eq!(read_key(&socket), None, "a key others can read is no key");
        drop_key(&socket);
        assert_eq!(read_key(&socket), None);
    }

    #[test]
    fn keys_compare_by_content_alone() {
        assert!(same_key("ab12", "ab12"));
        assert!(!same_key("ab12", "ab13"));
        assert!(!same_key("ab1", "ab12"));
    }
}
