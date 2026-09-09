//! The family socket: four bytes of big-endian length, then the body. Between siblings, with a
//! reply shape fixed for the whole family: `{"ok":true,"family":1,"n":N,"result":[…]}`, where
//! `result` is a *list* of return values. Calls go out in JSON unless [`Family::speaking`] says
//! otherwise, and replies are read in whichever encoding they arrive in. A refusal is a reply;
//! only the transport failing closes anything.

use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// The largest reply this client will read.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Where a call ended up.
#[derive(Debug, thiserror::Error)]
pub enum Fault {
    /// Nothing answered: no socket, a dead socket, or the connection died mid-call.
    #[error("balthasar is not reachable: {0}")]
    Unavailable(String),

    /// The verb was declined. Costs a feature; the caller carries on.
    #[error("balthasar refused: {0}")]
    Refused(String),

    /// The write did not land. What was handed over is not recorded.
    #[error("balthasar did not record it: {0}")]
    Failed(String),

    #[error("balthasar answered something unreadable: {0}")]
    Malformed(String),
}

impl Fault {
    /// Whether continuing would build on a hole.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Fault::Unavailable(_) | Fault::Failed(_))
    }
}

/// One held connection: balthasar serves many calls per connection, so the stream is kept.
pub struct Family {
    stream: UnixStream,
    scratch: Vec<u8>,
    /// Which encoding calls go out in; replies are read in whichever came back. JSON by default.
    wire: crate::Wire,
    path: PathBuf,
}

impl Family {
    /// Connect to a socket by path.
    pub async fn dial(path: impl AsRef<Path>) -> Result<Self, Fault> {
        let path = path.as_ref().to_path_buf();
        let stream = UnixStream::connect(&path)
            .await
            .map_err(|e| Fault::Unavailable(format!("{}: {e}", path.display())))?;
        Ok(Self {
            stream,
            scratch: Vec::new(),
            wire: crate::Wire::default(),
            path,
        })
    }

    /// Connect to whichever socket [`candidates`] offers first, newest wins. Each is tried in turn:
    /// a socket file left by a killed frontend looks like a live one until something connects to it.
    pub async fn find(dir: Option<&Path>) -> Result<Self, Fault> {
        let mut last = None;
        for path in candidates(dir) {
            match Self::dial(&path).await {
                Ok(open) => return Ok(open),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| Fault::Unavailable("no socket to try".into())))
    }

    /// The socket this is connected to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Send calls in `wire` from here on. Replies are read in whichever encoding they arrive in.
    #[must_use]
    pub fn speaking(mut self, wire: crate::Wire) -> Self {
        self.wire = wire;
        self
    }

    /// Send one call and wait for its answer.
    pub async fn call(
        &mut self,
        verb: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let mut body = serde_json::Map::new();
        body.insert("call".into(), serde_json::Value::String(verb.to_owned()));
        if !args.is_empty() {
            body.insert("args".into(), serde_json::Value::Array(args));
        }
        let body = self
            .wire
            .write(&serde_json::Value::Object(body))
            .map_err(|why| Fault::Malformed(format!("encoding {verb}: {why}")))?;

        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&body);
        self.stream
            .write_all(&frame)
            .await
            .map_err(|e| Fault::Unavailable(format!("sending {verb}: {e}")))?;

        // A socket that accepts the connection and then never replies is the ordinary shape of a
        // wedged process, so every asynchronous call is on a clock here rather than at the caller.
        tokio::time::timeout(PATIENCE, self.read_reply(verb))
            .await
            .map_err(|_| Fault::Unavailable(format!("{verb}: no answer in {PATIENCE:?}")))?
    }

    /// Read one framed reply and unwrap the family's envelope.
    async fn read_reply(&mut self, verb: &str) -> Result<Vec<serde_json::Value>, Fault> {
        let mut head = [0_u8; 4];
        self.stream
            .read_exact(&mut head)
            .await
            .map_err(|e| Fault::Unavailable(format!("awaiting {verb}: {e}")))?;

        let len = u32::from_be_bytes(head) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(Fault::Malformed(format!("{len} byte reply to {verb}")));
        }
        self.scratch.clear();
        self.scratch.resize(len, 0);
        self.stream
            .read_exact(&mut self.scratch)
            .await
            .map_err(|e| Fault::Unavailable(format!("reading {verb}: {e}")))?;

        // Read in whichever encoding came back rather than in the one we asked in.
        let reply: serde_json::Value = crate::Wire::read(&self.scratch)
            .map_err(|e| Fault::Malformed(format!("{verb}: {e}")))?;
        unwrap(&reply, verb)
    }
}

/// The newest revision of the family wire this understands, duplicated in each sibling.
pub const FAMILY: u16 = 1;

/// How long a call waits for its answer. The same number the blocking half uses.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(2000);

/// Split a reply into its return values, or into the fault it names. No `fault` field means
/// `refused`, the answer that costs a feature rather than a turn.
fn unwrap(reply: &serde_json::Value, verb: &str) -> Result<Vec<serde_json::Value>, Fault> {
    let Some(object) = reply.as_object() else {
        return Err(Fault::Malformed(format!("{verb}: reply is not an object")));
    };

    // A newer peer is refused by name here, where the reason is still known; a reply with no
    // `family` predates the field and is read as it always was.
    let spoken = object
        .get("family")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if spoken > u64::from(FAMILY) {
        return Err(Fault::Malformed(format!(
            "{verb}: this peer speaks version {spoken} of the family wire and this build \
             understands {FAMILY}; upgrade magi"
        )));
    }

    if object.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let why = object
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no reason given")
            .to_owned();
        return match object.get("fault").and_then(serde_json::Value::as_str) {
            Some("failed") => Err(Fault::Failed(why)),
            _ => Err(Fault::Refused(why)),
        };
    }

    let Some(values) = object.get("result") else {
        return Ok(Vec::new());
    };
    let Some(values) = values.as_array() else {
        return Err(Fault::Malformed(format!("{verb}: result is not a list")));
    };

    let n = object
        .get("n")
        .and_then(serde_json::Value::as_u64)
        .map_or(values.len(), |n| n as usize);
    Ok(values.iter().take(n).cloned().collect())
}

/// The directory balthasar binds its sockets in: `$XDG_RUNTIME_DIR/balthasar`, else a uid-suffixed
/// temp directory, with `$MAGI_BALTHASAR_INSTANCE` selecting one when several are running.
#[must_use]
pub fn socket_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join("balthasar"),
        None => {
            std::env::temp_dir().join(format!("balthasar-{}", rustix::process::getuid().as_raw()))
        }
    };
    match std::env::var("MAGI_BALTHASAR_INSTANCE") {
        Ok(instance) if !instance.is_empty() => base.join(instance),
        _ => base,
    }
}

/// Every socket worth trying, newest first. `$MAGI_API_SOCKET` alone when it is set: a program
/// balthasar started inherits it and means *that* session.
#[must_use]
pub fn candidates(dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(named) = std::env::var_os("MAGI_API_SOCKET").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(named)];
    }
    listing(&dir.map_or_else(socket_dir, Path::to_path_buf))
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
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
        .collect();

    // Newest first, so the key is reversed rather than the ordering.
    found.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    found.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    /// A peer from the future is refused by name; one from before versions is not.
    #[test]
    fn a_reply_from_a_newer_wire_is_refused_and_says_why() {
        let ahead = serde_json::json!({
            "ok": true, "family": super::FAMILY as u64 + 1, "n": 0, "result": []
        });
        let why = super::unwrap(&ahead, "verbs").expect_err("a newer peer is refused");
        let said = why.to_string();
        assert!(said.contains("family wire"), "{said}");
        assert!(said.contains("upgrade"), "it says what to do: {said}");
    }

    #[test]
    fn a_reply_from_before_versions_is_read_as_it_always_was() {
        // Every peer built before this field existed.
        let old = serde_json::json!({ "ok": true, "n": 1, "result": ["hello"] });
        let values = super::unwrap(&old, "verbs").expect("an older peer still answers");
        assert_eq!(values, vec![serde_json::json!("hello")]);
    }

    #[test]
    fn a_reply_from_this_wire_is_read() {
        let now = serde_json::json!({
            "ok": true, "family": super::FAMILY, "n": 1, "result": ["hello"]
        });
        assert_eq!(
            super::unwrap(&now, "verbs").expect("read"),
            vec![serde_json::json!("hello")]
        );
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn a_successful_reply_yields_its_result_list() {
        let reply = json!({ "ok": true, "n": 2, "result": ["a", "b"] });
        let values = unwrap(&reply, "replay").expect("ok");
        assert_eq!(values, vec![json!("a"), json!("b")]);
    }

    #[test]
    fn n_bounds_the_result_rather_than_its_length() {
        let reply = json!({ "ok": true, "n": 1, "result": ["a", "b"] });
        assert_eq!(unwrap(&reply, "replay").expect("ok"), vec![json!("a")]);
    }

    #[test]
    fn a_bare_ok_is_no_return_values_rather_than_an_error() {
        let reply = json!({ "ok": true, "n": 0 });
        assert!(unwrap(&reply, "observe").expect("ok").is_empty());
    }

    #[test]
    fn a_failed_write_is_told_apart_from_a_refusal() {
        let refused = json!({ "ok": false, "error": "no such verb" });
        assert!(matches!(unwrap(&refused, "plan"), Err(Fault::Refused(_))));

        let failed = json!({ "ok": false, "error": "disk full", "fault": "failed" });
        assert!(matches!(unwrap(&failed, "observe"), Err(Fault::Failed(_))));
    }

    #[test]
    fn only_the_two_that_cost_a_turn_are_fatal() {
        assert!(Fault::Failed("x".into()).is_fatal());
        assert!(Fault::Unavailable("x".into()).is_fatal());
        assert!(!Fault::Refused("x".into()).is_fatal());
        assert!(!Fault::Malformed("x".into()).is_fatal());
    }

    #[test]
    fn a_reply_that_is_not_the_agreed_shape_is_malformed_not_refused() {
        assert!(matches!(
            unwrap(&json!(["a"]), "replay"),
            Err(Fault::Malformed(_))
        ));
        assert!(matches!(
            unwrap(&json!({ "ok": true, "result": "a" }), "replay"),
            Err(Fault::Malformed(_))
        ));
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

/// The same protocol, without a runtime, for the synchronous paths such as the session picker.
pub mod blocking {
    use super::{Fault, candidates, unwrap};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    /// How long a picker will wait before drawing without balthasar.
    const PATIENCE: Duration = Duration::from_millis(2000);

    /// One held connection.
    pub struct Family {
        stream: UnixStream,
    }

    impl Family {
        /// Connect to a socket by path.
        pub fn dial(path: impl AsRef<Path>) -> Result<Self, Fault> {
            let path = path.as_ref();
            let stream = UnixStream::connect(path)
                .map_err(|e| Fault::Unavailable(format!("{}: {e}", path.display())))?;
            let _ = stream.set_read_timeout(Some(PATIENCE));
            let _ = stream.set_write_timeout(Some(PATIENCE));
            Ok(Self { stream })
        }

        /// Connect to whichever socket answers first, newest tried first. Answers, not accepts: the
        /// kernel accepts on behalf of a listener whose owner has stopped reading, so each candidate
        /// is proved with one `verbs` call before the rest are given up on.
        pub fn find() -> Result<Self, Fault> {
            let mut last = None;
            for path in candidates(None) {
                match Self::dial(&path) {
                    Ok(mut open) => match open.call("verbs", Vec::new()) {
                        Ok(_) => return Ok(open),
                        Err(e) => last = Some(e),
                    },
                    Err(e) => last = Some(e),
                }
            }
            Err(last.unwrap_or_else(|| Fault::Unavailable("no socket to try".into())))
        }

        /// Send one call and wait for its answer.
        pub fn call(
            &mut self,
            verb: &str,
            args: Vec<serde_json::Value>,
        ) -> Result<Vec<serde_json::Value>, Fault> {
            let mut body = serde_json::Map::new();
            body.insert("call".into(), serde_json::Value::String(verb.to_owned()));
            if !args.is_empty() {
                body.insert("args".into(), serde_json::Value::Array(args));
            }
            let body = serde_json::Value::Object(body).to_string();

            let mut frame = Vec::with_capacity(4 + body.len());
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frame.extend_from_slice(body.as_bytes());
            self.stream
                .write_all(&frame)
                .map_err(|e| Fault::Unavailable(format!("sending {verb}: {e}")))?;

            let mut head = [0_u8; 4];
            self.stream
                .read_exact(&mut head)
                .map_err(|e| Fault::Unavailable(format!("awaiting {verb}: {e}")))?;
            let len = u32::from_be_bytes(head) as usize;
            if len > super::MAX_FRAME_BYTES {
                return Err(Fault::Malformed(format!("{len} byte reply to {verb}")));
            }
            let mut rest = vec![0_u8; len];
            self.stream
                .read_exact(&mut rest)
                .map_err(|e| Fault::Unavailable(format!("reading {verb}: {e}")))?;

            // In whichever encoding came back, as in the asynchronous half above.
            let reply: serde_json::Value =
                crate::Wire::read(&rest).map_err(|e| Fault::Malformed(format!("{verb}: {e}")))?;
            unwrap(&reply, verb)
        }
    }
}
