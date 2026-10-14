//! The same protocol, without a runtime, for the synchronous paths such as the session picker.

use super::{Fault, candidates, frame, named, served_by_us, unwrap};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a picker will wait before drawing without balthasar, on a write and on a read.
const PATIENCE: Duration = Duration::from_millis(2000);

/// One held connection, taken out for each call and put back only when a whole reply came back:
/// a call that failed takes the connection with it, and the next call dials again.
pub struct Family {
    stream: Option<UnixStream>,
    path: PathBuf,
    key: Option<super::keys::Key>,
}

/// Open a stream with both deadlines set, served by this user or not at all.
fn opened(path: &Path) -> Result<UnixStream, Fault> {
    let stream = UnixStream::connect(path)
        .map_err(|e| Fault::Unavailable(format!("{}: {e}", path.display())))?;
    served_by_us(&stream, path)?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));
    Ok(stream)
}

impl Family {
    /// Connect to a socket by path. Every call carries the key held for it, if one is.
    pub fn dial(path: impl AsRef<Path>) -> Result<Self, Fault> {
        let path = path.as_ref();
        Ok(Self {
            stream: Some(opened(path)?),
            path: path.to_path_buf(),
            key: super::keys::held_for(path),
        })
    }

    /// Connect to this process's own memory layer, or the one `$MAGI_API_SOCKET` names, and to
    /// nothing else: what changes a store must reach the store this session records into.
    pub fn ours() -> Result<Self, Fault> {
        let path = named().ok_or_else(|| {
            Fault::Unavailable("this process has not convened a memory layer".into())
        })?;
        Self::dial(path)
    }

    /// Connect to whichever socket answers first, newest tried first; only this process's own
    /// when it has one. Each candidate is proved with one `verbs` call.
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

    /// Send one call and wait for its answer. A call over the frame cap is refused here and
    /// never sent.
    pub fn call(
        &mut self,
        verb: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let frame = frame(crate::Wire::Json, verb, &args, self.key.as_ref())?;
        let mut stream = match self.stream.take() {
            Some(stream) => stream,
            None => opened(&self.path)?,
        };
        stream
            .write_all(&frame)
            .map_err(|e| Fault::Unavailable(format!("sending {verb}: {e}")))?;
        let reply = read_reply(&mut stream, verb);
        // A refusal is a whole reply, and leaves nothing owed on the stream.
        if matches!(&reply, Ok(_) | Err(Fault::Refused(_) | Fault::Failed(_))) {
            self.stream = Some(stream);
        }
        reply
    }
}

/// Read one framed reply and unwrap the family's envelope. A declared length of nothing, or past
/// the cap, is refused before anything is allocated for it.
fn read_reply(stream: &mut UnixStream, verb: &str) -> Result<Vec<serde_json::Value>, Fault> {
    let mut head = [0_u8; 4];
    stream
        .read_exact(&mut head)
        .map_err(|e| Fault::Unavailable(format!("awaiting {verb}: {e}")))?;
    let len = u32::from_be_bytes(head) as usize;
    if len == 0 || len > super::MAX_FRAME_BYTES {
        return Err(Fault::Malformed(format!(
            "a {len} byte reply to {verb}; a frame carries 1 to {}",
            super::MAX_FRAME_BYTES
        )));
    }
    let mut rest = vec![0_u8; len];
    stream
        .read_exact(&mut rest)
        .map_err(|e| Fault::Unavailable(format!("reading {verb}: {e}")))?;
    let reply: serde_json::Value =
        crate::Wire::read(&rest).map_err(|e| Fault::Malformed(format!("{verb}: {e}")))?;
    unwrap(&reply, verb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A peer that answers each call with its verb, the first one too late to be waited for.
    fn echoing(path: &Path) -> std::thread::JoinHandle<()> {
        let listener = std::os::unix::net::UnixListener::bind(path).expect("bind");
        std::thread::spawn(move || {
            for nth in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut head = [0_u8; 4];
                if stream.read_exact(&mut head).is_err() {
                    return;
                }
                let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
                if stream.read_exact(&mut body).is_err() {
                    return;
                }
                let asked: serde_json::Value = serde_json::from_slice(&body).expect("json");
                let verb = asked["call"].as_str().unwrap_or_default().to_owned();
                if nth == 0 {
                    std::thread::sleep(PATIENCE + Duration::from_millis(500));
                }
                let reply = json!({ "ok": true, "n": 1, "result": [verb] }).to_string();
                let mut framed = (reply.len() as u32).to_be_bytes().to_vec();
                framed.extend_from_slice(reply.as_bytes());
                let _ = stream.write_all(&framed);
            }
        })
    }

    #[test]
    fn a_call_that_timed_out_does_not_answer_the_next_one() {
        let dir = magi_model::scratch::Scratch::new("magi-blocking-adrift", "one");
        let path = dir.join("api@one.sock");
        let served = echoing(&path);

        let mut open = Family::dial(&path).expect("dial");
        assert!(
            open.call("plan", Vec::new()).is_err(),
            "the first call timed out"
        );
        let answered = open.call("observe", Vec::new()).expect("observe");
        assert_eq!(
            answered,
            vec![json!("observe")],
            "it read the abandoned reply"
        );
        drop(open);
        let _ = served.join();
    }

    #[test]
    fn a_call_over_the_cap_is_refused_before_a_byte_is_sent() {
        let dir = magi_model::scratch::Scratch::new("magi-blocking-cap", "one");
        let path = dir.join("api@cap.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let mut open = Family::dial(&path).expect("dial");
        let (mut accepted, _) = listener.accept().expect("accept");

        let huge = json!("x".repeat(super::super::MAX_FRAME_BYTES));
        let refused = open.call("observe", vec![huge]).expect_err("refused");
        assert!(refused.to_string().contains("frame cap"), "{refused}");
        accepted.set_nonblocking(true).expect("nonblocking fixture");
        let mut byte = [0_u8; 1];
        assert!(
            accepted.read(&mut byte).is_err(),
            "something was written for a call that was refused"
        );
    }
}
