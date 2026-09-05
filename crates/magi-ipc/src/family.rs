//! The family socket: four bytes of big-endian length, then a JSON body.
//!
//! The same framing as magi's own codec over a different encoding, and a different contract.
//! magi's own wire is CBOR between a UI and its session; this one is JSON between siblings, and
//! its reply shape is fixed for the whole family: `{"ok":true,"n":N,"result":[…]}`, where
//! `result` is a *list* of return values.
//!
//! A refusal is a reply. Only the transport failing closes anything, which is what lets
//! [`Fault`] tell "balthasar said no" from "balthasar is not there".

use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// The largest reply this client will read.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Where a call ended up.
///
/// Three, not two. A caller that cannot tell these apart either carries on after losing a turn
/// or gives up over a missing feature.
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

    /// The reply was not the shape the family agreed on.
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

/// One held connection.
///
/// balthasar serves many calls per connection and is asked several times in a turn, so the
/// stream is kept rather than redialled.
pub struct Family {
    stream: UnixStream,
    scratch: Vec<u8>,
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
            path,
        })
    }

    /// Connect to whichever socket [`candidates`] offers first, newest wins.
    ///
    /// Each is tried in turn: a socket file left by a killed frontend looks exactly like a live
    /// one until something connects to it.
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
        let body = serde_json::Value::Object(body).to_string();

        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(body.as_bytes());
        self.stream
            .write_all(&frame)
            .await
            .map_err(|e| Fault::Unavailable(format!("sending {verb}: {e}")))?;

        self.read_reply(verb).await
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

        let reply: serde_json::Value = serde_json::from_slice(&self.scratch)
            .map_err(|e| Fault::Malformed(format!("{verb}: {e}")))?;
        unwrap(&reply, verb)
    }
}

/// Split a reply into its return values, or into the fault it names.
///
/// `fault` distinguishes the two refusals; its absence means `refused`, which is the answer that
/// costs a feature rather than a turn.
fn unwrap(reply: &serde_json::Value, verb: &str) -> Result<Vec<serde_json::Value>, Fault> {
    let Some(object) = reply.as_object() else {
        return Err(Fault::Malformed(format!("{verb}: reply is not an object")));
    };

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

/// The directory balthasar binds its sockets in.
///
/// `$XDG_RUNTIME_DIR/balthasar`, else a uid-suffixed temp directory, with
/// `$MAGI_BALTHASAR_INSTANCE` selecting one when several are running.
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

/// Every socket worth trying, newest first.
///
/// `$MAGI_API_SOCKET` alone when it is set: a program balthasar started inherits it and means
/// *that* session, so there is nothing to guess.
#[must_use]
pub fn candidates(dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(named) = std::env::var_os("MAGI_API_SOCKET").filter(|v| !v.is_empty()) {
        return vec![PathBuf::from(named)];
    }
    listing(&dir.map_or_else(socket_dir, Path::to_path_buf))
}

/// Every `api@*.sock` in one directory, newest first.
///
/// The directory and nothing else — no `$MAGI_API_SOCKET`, no default location. [`candidates`]
/// answers "which one should I talk to", which an inherited variable settles outright; this
/// answers "which are there", which is what a sweep needs and what a test can check without
/// setting a process-wide variable that Rust 2024 makes `unsafe` and this crate denies.
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

    // Newest first, so `by_key` on the key alone would put it the wrong way round; reversing the
    // key is what clippy asks for here and it says the same thing.
    found.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    found.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
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
        // Named after this process. A fixed path under a shared directory is one collision away
        // from two test binaries deleting each other's fixture.
        let dir = std::env::temp_dir().join(format!("magi-family-listing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for name in ["api@old.sock", "api@new.sock", "notes.txt", "api@x.other"] {
            std::fs::write(dir.join(name), b"").expect("write");
        }
        // The gap is *set*, not hoped for. Writing one file after another and trusting the two
        // mtimes to differ works on a laptop and fails on a fast machine, where both land in the
        // same filesystem tick: the sort is stable, so equal times leave `read_dir` order, which
        // is arbitrary. CI failed on exactly that.
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The same protocol, without a runtime.
///
/// The UI asks balthasar what sessions there are while drawing a picker, and that path is
/// synchronous: a blocking dial is the honest shape for it rather than borrowing a runtime.
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

        /// Connect to whichever socket answers first, newest tried first.
        pub fn find() -> Result<Self, Fault> {
            let mut last = None;
            for path in candidates(None) {
                match Self::dial(&path) {
                    Ok(open) => return Ok(open),
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

            let reply: serde_json::Value = serde_json::from_slice(&rest)
                .map_err(|e| Fault::Malformed(format!("{verb}: {e}")))?;
            unwrap(&reply, verb)
        }
    }
}
