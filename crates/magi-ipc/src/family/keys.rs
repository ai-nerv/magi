//! The keys this process handed the memory layers it started, by the socket each listens on. A
//! call to one of those sockets carries its key; a call to any other carries none.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One key, never printed: its `Debug` says only that it is there.
#[derive(Clone)]
pub(super) struct Key(String);

impl Key {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

/// Every key held, with the socket it is for.
static HELD: Mutex<Vec<(PathBuf, Key)>> = Mutex::new(Vec::new());

/// Hold `key` for the memory layer listening on `socket`, in place of any held for it before.
pub fn hold_key(socket: &Path, key: &str) {
    let Ok(mut held) = HELD.lock() else {
        return;
    };
    held.retain(|(path, _)| path != socket);
    held.push((socket.to_path_buf(), Key(key.to_owned())));
}

/// The key held for `socket`, matched as it is named or as it resolves.
pub(super) fn held_for(socket: &Path) -> Option<Key> {
    let held = HELD.lock().ok()?;
    if let Some((_, key)) = held.iter().find(|(path, _)| path == socket) {
        return Some(key.clone());
    }
    let wanted = resolved(socket)?;
    held.iter()
        .find(|(path, _)| resolved(path).as_ref() == Some(&wanted))
        .map(|(_, key)| key.clone())
}

/// Whether a key is held for `socket`.
#[must_use]
pub fn holds_key(socket: &Path) -> bool {
    held_for(socket).is_some()
}

/// `socket` with its directory resolved, so two spellings of one path compare equal.
fn resolved(socket: &Path) -> Option<PathBuf> {
    Some(
        socket
            .parent()?
            .canonicalize()
            .ok()?
            .join(socket.file_name()?),
    )
}

/// Add the key held for `socket` to `call`, an object, when one is held.
pub fn seal(socket: &Path, call: &mut serde_json::Value) {
    if let (Some(key), Some(fields)) = (held_for(socket), call.as_object_mut()) {
        fields.insert("key".to_owned(), serde_json::Value::from(key.as_str()));
    }
}

/// `frame`, one whole frame whose body is a call, framed again with the key held for `socket`
/// added in the body's own encoding. `None` when no key is held for it, or when the bytes are not
/// one such frame or would not fit one once sealed.
#[must_use]
pub fn sealed(socket: &Path, frame: &[u8]) -> Option<Vec<u8>> {
    let key = held_for(socket)?;
    let (head, body) = frame.split_first_chunk::<4>()?;
    if body.is_empty() || u32::from_be_bytes(*head) as usize != body.len() {
        return None;
    }
    let wire = crate::Wire::of(body);
    let mut call: serde_json::Value = crate::Wire::read(body).ok()?;
    call.as_object_mut()?
        .insert("key".to_owned(), serde_json::Value::from(key.as_str()));
    let body = wire.write(&call).ok()?;
    let length = u32::try_from(body.len())
        .ok()
        .filter(|n| *n as usize <= super::MAX_FRAME_BYTES)?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    fn framed(body: &serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_vec(body).expect("encode");
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn a_key_goes_only_to_the_socket_it_was_held_for() {
        let dir = magi_model::scratch::Scratch::new("magi-keys", "only");
        let ours = dir.join("api@ours.sock");
        hold_key(&ours, KEY);
        let call = framed(&serde_json::json!({ "call": "verbs" }));

        let sealed_ours = sealed(&ours, &call).expect("sealed for ours");
        let body: serde_json::Value =
            serde_json::from_slice(sealed_ours.get(4..).expect("a body")).expect("json");
        assert_eq!(body["key"], KEY);
        assert_eq!(body["call"], "verbs");

        assert!(sealed(&dir.join("api@theirs.sock"), &call).is_none());
        let mut plain = serde_json::json!({ "call": "verbs" });
        seal(&dir.join("melchior.sock"), &mut plain);
        assert!(plain.get("key").is_none(), "{plain}");
        seal(&dir.join(".").join("api@ours.sock"), &mut plain);
        assert_eq!(plain["key"], KEY, "another spelling of the same socket");
        assert!(!format!("{:?}", held_for(&ours)).contains("ab12"));
    }

    #[test]
    fn what_is_not_one_whole_call_is_left_as_it_is() {
        let dir = magi_model::scratch::Scratch::new("magi-keys", "whole");
        let ours = dir.join("api@ours.sock");
        hold_key(&ours, KEY);
        let call = framed(&serde_json::json!({ "call": "verbs" }));
        assert!(sealed(&ours, call.get(..call.len() - 1).expect("short")).is_none());
        assert!(sealed(&ours, &framed(&serde_json::json!(["a list"]))).is_none());
        assert!(sealed(&ours, &[0, 0, 0, 0]).is_none());
    }
}
