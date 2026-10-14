//! The family socket: four bytes of big-endian length, then the body. Between siblings, with a
//! reply shape fixed for the whole family: `{"ok":true,"family":1,"n":N,"result":[…]}`, where
//! `result` is a *list* of return values. Calls go out in JSON unless [`Family::speaking`] says
//! otherwise, and replies are read in whichever encoding they arrive in. A refusal is a reply;
//! only the transport failing closes anything.

use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// The family's one frame cap, on a call this client writes and on a reply it reads.
pub const MAX_FRAME_BYTES: usize = crate::MAX_FRAME_BYTES;

/// One call as it goes on the wire: four bytes of length, then the body.
#[derive(serde::Serialize)]
struct Call<'a> {
    call: &'a str,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    args: &'a [serde_json::Value],
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
}

/// A call framed for sending, with the key held for its socket when there is one, or refused here
/// when its body is past [`MAX_FRAME_BYTES`].
fn frame(
    wire: crate::Wire,
    verb: &str,
    args: &[serde_json::Value],
    key: Option<&keys::Key>,
) -> Result<Vec<u8>, Fault> {
    let key = key.map(keys::Key::as_str);
    let body = wire
        .write(&Call {
            call: verb,
            args,
            key,
        })
        .map_err(|why| Fault::Malformed(format!("encoding {verb}: {why}")))?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|len| *len as usize <= MAX_FRAME_BYTES)
        .ok_or_else(|| too_large(verb, body.len()))?;
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Whether a call of `verb` with `args` fits in one frame with a key beside it, measured in JSON,
/// the larger encoding.
#[must_use]
pub fn fits(verb: &str, args: &[serde_json::Value]) -> bool {
    const ROOM_FOR_A_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    crate::Wire::Json
        .write(&Call {
            call: verb,
            args,
            key: Some(ROOM_FOR_A_KEY),
        })
        .is_ok_and(|body| body.len() <= MAX_FRAME_BYTES)
}

/// The refusal for a call that would not fit in one frame. Nothing was sent.
fn too_large(verb: &str, len: usize) -> Fault {
    Fault::Failed(format!(
        "{verb} is {len} bytes, past the family's {MAX_FRAME_BYTES} byte frame cap; it was not sent"
    ))
}

/// Close what another user is serving: a socket path is a name, and the kernel says who holds it.
fn served_by_us(stream: &impl std::os::fd::AsFd, path: &Path) -> Result<(), Fault> {
    let peer = crate::peer_uid(stream)
        .map_err(|e| Fault::Unavailable(format!("{}: {e}", path.display())))?;
    if peer != crate::uid() {
        return Err(Fault::Unavailable(format!(
            "{} is served by uid {peer}, not this user",
            path.display()
        )));
    }
    Ok(())
}

/// Connect, and keep the stream only when this user is the one serving it.
async fn connected(path: &Path) -> Result<UnixStream, Fault> {
    let stream = UnixStream::connect(path)
        .await
        .map_err(|e| Fault::Unavailable(format!("{}: {e}", path.display())))?;
    served_by_us(&stream, path)?;
    Ok(stream)
}

/// Where a call ended up.
#[derive(Debug, thiserror::Error)]
pub enum Fault {
    /// Nothing answered: no socket, a dead socket, or the connection died mid-call.
    #[error("the memory layer is not reachable: {0}")]
    Unavailable(String),

    /// The verb was declined. Costs a feature; the caller carries on.
    #[error("the memory layer refused: {0}")]
    Refused(String),

    /// The write did not land. What was handed over is not recorded.
    #[error("the memory layer did not record it: {0}")]
    Failed(String),

    #[error("the memory layer answered something unreadable: {0}")]
    Malformed(String),
}

impl Fault {
    /// Whether continuing would build on a hole.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Fault::Unavailable(_) | Fault::Failed(_))
    }
}

/// One held connection: balthasar serves many calls per connection, so the stream is kept between
/// calls. Each call takes it out and puts it back only when a whole reply came back; a call that
/// failed, or was dropped where it stood, takes the connection with it, and the next call dials
/// again. One reply per call, in order, so an abandoned reply is never read as the next answer.
pub struct Family {
    stream: Option<UnixStream>,
    scratch: Vec<u8>,
    /// Which encoding calls go out in; replies are read in whichever came back. JSON by default.
    wire: crate::Wire,
    path: PathBuf,
    /// What every call carries, when this process holds a key for the socket.
    key: Option<keys::Key>,
}

impl Family {
    /// Connect to a socket by path. Every call carries the key held for it, if one is.
    pub async fn dial(path: impl AsRef<Path>) -> Result<Self, Fault> {
        let path = path.as_ref().to_path_buf();
        let stream = connected(&path).await?;
        Ok(Self {
            stream: Some(stream),
            scratch: Vec::new(),
            wire: crate::Wire::default(),
            key: keys::held_for(&path),
            path,
        })
    }

    /// Connect to the newest socket in `dir` that takes the connection. With no directory, only
    /// this process's own memory layer or the one `$MAGI_API_SOCKET` names: never a neighbour's.
    pub async fn find(dir: Option<&Path>) -> Result<Self, Fault> {
        let mut last = None;
        let paths = match dir {
            Some(_) => candidates(dir),
            None => named().into_iter().collect(),
        };
        for path in paths {
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
        self.call_within(verb, args, PATIENCE).await
    }

    /// The same, waiting `patience` rather than the clock a feature keeps. For the calls whose
    /// failure costs the conversation rather than a feature — see [`DURABLE`].
    ///
    /// A call refused for its key is made once more with the key the memory layer now publishes
    /// beside its socket, when that is a different one: the layer was started again by another.
    pub async fn call_within(
        &mut self,
        verb: &str,
        args: Vec<serde_json::Value>,
        patience: std::time::Duration,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let reply = self.once(verb, &args, patience).await;
        if matches!(&reply, Err(Fault::Refused(why)) if why == keys::REFUSED)
            && let Some(fresh) = keys::published(&self.path)
            && self.key.as_ref().map(keys::Key::as_str) != Some(fresh.as_str())
        {
            self.key = Some(fresh);
            return self.once(verb, &args, patience).await;
        }
        reply
    }

    /// One call and its reply, the connection kept only when the reply came back whole.
    async fn once(
        &mut self,
        verb: &str,
        args: &[serde_json::Value],
        patience: std::time::Duration,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let frame = frame(self.wire, verb, args, self.key.as_ref())?;
        let mut stream = match self.stream.take() {
            Some(stream) => stream,
            None => connected(&self.path).await?,
        };
        tokio::time::timeout(patience, stream.write_all(&frame))
            .await
            .map_err(|_| Fault::Unavailable(format!("sending {verb}: not taken in {patience:?}")))?
            .map_err(|e| Fault::Unavailable(format!("sending {verb}: {e}")))?;

        // A socket that accepts the connection and then never replies is the ordinary shape of a
        // wedged process, so every asynchronous call is on a clock here rather than at the caller.
        let reply =
            tokio::time::timeout(patience, read_reply(&mut stream, &mut self.scratch, verb))
                .await
                .map_err(|_| Fault::Unavailable(format!("{verb}: no answer in {patience:?}")))?;
        // A refusal is a whole reply, and leaves nothing owed on the stream.
        if matches!(&reply, Ok(_) | Err(Fault::Refused(_) | Fault::Failed(_))) {
            self.stream = Some(stream);
        }
        reply
    }
}

/// Read one framed reply and unwrap the family's envelope. A declared length of nothing, or past
/// the cap, is refused before anything is allocated for it.
async fn read_reply(
    stream: &mut UnixStream,
    scratch: &mut Vec<u8>,
    verb: &str,
) -> Result<Vec<serde_json::Value>, Fault> {
    let mut head = [0_u8; 4];
    stream
        .read_exact(&mut head)
        .await
        .map_err(|e| Fault::Unavailable(format!("awaiting {verb}: {e}")))?;

    let len = u32::from_be_bytes(head) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(Fault::Malformed(format!(
            "a {len} byte reply to {verb}; a frame carries 1 to {MAX_FRAME_BYTES}"
        )));
    }
    scratch.clear();
    scratch.resize(len, 0);
    stream
        .read_exact(scratch)
        .await
        .map_err(|e| Fault::Unavailable(format!("reading {verb}: {e}")))?;

    // Read in whichever encoding came back rather than in the one we asked in.
    let reply: serde_json::Value =
        crate::Wire::read(scratch).map_err(|e| Fault::Malformed(format!("{verb}: {e}")))?;
    unwrap(&reply, verb)
}

/// The newest revision of the family wire this understands, duplicated in each sibling.
pub const FAMILY: u16 = 1;

/// [`FAMILY`], for serde's `default`.
fn family() -> u16 {
    FAMILY
}

/// Which kind of no a reply carries. Absent on the wire means [`Faulted::Refused`], the answer
/// that costs a feature rather than a turn. Named apart from [`Fault`], which is what a *call*
/// ended up as on this side rather than what a reply says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Faulted {
    Refused,
    Failed,
}

/// One reply, in the shape FAMILY.md fixes for every program in the family. Built here rather
/// than assembled by hand at each site: a hand-built envelope agrees with the contract only for
/// as long as nobody edits it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Reply {
    pub ok: bool,
    /// Defaulted on the way in, so a reply from a build older than this field still parses.
    #[serde(default = "family")]
    pub family: u16,
    /// Only ever on `verbs`: a fact about the program rather than about the reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<u16>,
    /// Always `result.len()`.
    #[serde(default)]
    pub n: usize,
    /// The values, always a list: a listing is the rows, never one row that is the list.
    #[serde(default)]
    pub result: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<Faulted>,
}

impl Reply {
    /// An answer of one value.
    #[must_use]
    pub fn of(value: serde_json::Value) -> Self {
        Self::rows(vec![value])
    }

    /// An answer of several.
    #[must_use]
    pub fn rows(values: Vec<serde_json::Value>) -> Self {
        Self {
            ok: true,
            family: FAMILY,
            surface: None,
            n: values.len(),
            result: values,
            error: None,
            fault: None,
        }
    }

    /// An answer of none, for a verb that does something rather than reporting something.
    #[must_use]
    pub fn done() -> Self {
        Self::rows(Vec::new())
    }

    /// A no that costs a feature rather than a turn: `fault` stays absent, which means refused.
    #[must_use]
    pub fn refused(why: impl Into<String>) -> Self {
        Self {
            ok: false,
            family: FAMILY,
            surface: None,
            n: 0,
            result: Vec::new(),
            error: Some(why.into()),
            fault: None,
        }
    }

    /// What a third party writes against. Set on `verbs` and nowhere else.
    #[must_use]
    pub fn on_surface(mut self, surface: u16) -> Self {
        self.surface = Some(surface);
        self
    }

    /// This reply as bytes. A reply that will not encode as CBOR goes out as JSON rather than as
    /// nothing, since the caller is owed an answer either way.
    #[must_use]
    pub fn encode(&self, wire: crate::Wire) -> Vec<u8> {
        wire.write(self)
            .or_else(|_| crate::Wire::Json.write(self))
            .unwrap_or_else(|_| b"{\"ok\":false,\"family\":1,\"n\":0,\"result\":[]}".to_vec())
    }
}

/// How long a call waits for its answer. The same number the blocking half uses.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(2000);

/// How long a call carrying the transcript waits. balthasar opens a session's store on the first
/// call it is asked for it, which has been measured at ten seconds on a busy machine — and giving
/// up on a write costs the exchange, where giving up on a recall costs a feature.
pub const DURABLE: std::time::Duration = std::time::Duration::from_secs(30);

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
        return Err(Fault::Refused(format!(
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

pub mod blocking;
mod keys;
mod looking;
pub use keys::{hold_key, holds_key, seal, sealed};
pub use looking::{candidates, legacy_socket_dir, named, own, socket_dir, socket_dirs, sockets_in};

#[cfg(test)]
mod tests {
    /// A peer from the future is refused by name; one from before versions is not.
    #[test]
    fn a_reply_from_a_newer_wire_is_refused_and_says_why() {
        let ahead = serde_json::json!({
            "ok": true, "family": super::FAMILY as u64 + 1, "n": 0, "result": []
        });
        let why = super::unwrap(&ahead, "verbs").expect_err("a newer peer is refused");
        assert!(matches!(why, super::Fault::Refused(_)), "{why:?}");
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

    /// The shape FAMILY.md fixes, checked on the wire rather than on the struct.
    #[test]
    fn a_reply_carries_the_four_fields_every_answer_owes() {
        let wire = serde_json::to_value(Reply::rows(vec![json!("a"), json!("b")])).expect("enc");
        assert_eq!(wire["ok"], json!(true));
        assert_eq!(wire["family"], json!(FAMILY));
        assert_eq!(wire["n"], json!(2));
        assert_eq!(wire["result"], json!(["a", "b"]));
    }

    /// `n` and `result` are owed even when there is nothing to report: a caller reading `result`
    /// as a list must not have to tell absent from empty.
    #[test]
    fn an_answer_of_none_still_carries_an_empty_list() {
        let wire = serde_json::to_value(Reply::done()).expect("enc");
        assert_eq!(wire["n"], json!(0));
        assert_eq!(wire["result"], json!([]));
    }

    /// A listing is the rows, not one row that is the list.
    #[test]
    fn a_listing_is_the_rows() {
        let wire = serde_json::to_string(&Reply::rows(vec![json!({"verb": "verbs"})])).expect("e");
        assert!(!wire.contains(r#""result":[["#), "{wire}");
    }

    #[test]
    fn surface_is_on_verbs_and_nowhere_else() {
        let plain = serde_json::to_value(Reply::of(json!("x"))).expect("enc");
        assert!(plain.get("surface").is_none(), "{plain}");
        let listing = serde_json::to_value(Reply::rows(Vec::new()).on_surface(7)).expect("enc");
        assert_eq!(listing["surface"], json!(7));
    }

    /// A refusal names why, and says nothing about `fault`: absent means refused.
    #[test]
    fn a_refusal_names_why_and_leaves_fault_absent() {
        let wire = serde_json::to_value(Reply::refused("no such call: nope")).expect("enc");
        assert_eq!(wire["ok"], json!(false));
        assert_eq!(wire["error"], json!("no such call: nope"));
        assert!(wire.get("fault").is_none(), "{wire}");
        assert_eq!(wire["result"], json!([]));
    }

    #[test]
    fn a_reply_that_failed_says_so_by_name() {
        let mut failed = Reply::refused("disk full");
        failed.fault = Some(Faulted::Failed);
        let wire = serde_json::to_value(&failed).expect("enc");
        assert_eq!(wire["fault"], json!("failed"));
        // And it reads back as the fault that costs a turn.
        assert!(matches!(unwrap(&wire, "observe"), Err(Fault::Failed(_))));
    }

    /// A peer built before `family` existed still parses, and reads as revision 0.
    #[test]
    fn a_reply_from_before_the_field_parses_with_a_default() {
        let old: Reply = serde_json::from_value(json!({ "ok": true })).expect("parse");
        assert_eq!(old.family, FAMILY);
        assert!(old.result.is_empty());
    }

    #[test]
    fn one_shape_survives_both_encodings() {
        let reply = Reply::rows(vec![json!({"verb": "verbs"})]).on_surface(1);
        for wire in [crate::Wire::Json, crate::Wire::Cbor] {
            let back: Reply = crate::Wire::read(&reply.encode(wire)).expect("read");
            assert_eq!(back.surface, Some(1), "{wire:?}");
            assert_eq!(back.n, 1, "{wire:?}");
        }
    }

    #[tokio::test]
    async fn a_peer_that_never_reads_costs_a_call_and_not_the_session() {
        let dir = magi_model::scratch::Scratch::new("magi-family-unread", "one");
        let path = dir.join("api@unread.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bind");
        let mut open = Family::dial(&path).await.expect("dial");
        let (_held, _) = listener.accept().await.expect("accept");

        let big = json!("x".repeat(MAX_FRAME_BYTES / 2));
        let started = std::time::Instant::now();
        let sent = open
            .call_within("observe", vec![big], std::time::Duration::from_millis(300))
            .await;
        assert!(matches!(sent, Err(Fault::Unavailable(_))), "{sent:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn a_call_past_the_cap_is_refused_and_one_inside_it_is_framed() {
        let huge = vec![json!("x".repeat(MAX_FRAME_BYTES))];
        let refused = frame(crate::Wire::Json, "observe", &huge, None).expect_err("refused");
        assert!(matches!(refused, Fault::Failed(_)), "{refused:?}");
        assert!(refused.to_string().contains("frame cap"), "{refused}");

        let framed = frame(crate::Wire::Json, "verbs", &[], None).expect("framed");
        assert_eq!(
            &framed[4..],
            br#"{"call":"verbs"}"#,
            "no args, no args field"
        );
        assert_eq!(
            u32::from_be_bytes([framed[0], framed[1], framed[2], framed[3]]),
            16
        );
    }

    /// What a peer was sent, one call per connection, answered with its verb.
    fn recording(path: &Path, calls: usize) -> std::thread::JoinHandle<Vec<serde_json::Value>> {
        use std::io::{Read, Write};

        let listener = std::os::unix::net::UnixListener::bind(path).expect("bind");
        std::thread::spawn(move || {
            let mut heard = Vec::new();
            for _ in 0..calls {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut head = [0_u8; 4];
                stream.read_exact(&mut head).expect("head");
                let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
                stream.read_exact(&mut body).expect("body");
                let call: serde_json::Value = serde_json::from_slice(&body).expect("json");
                let reply = json!({ "ok": true, "n": 1, "result": [call["call"].clone()] });
                let reply = reply.to_string();
                let mut framed = (reply.len() as u32).to_be_bytes().to_vec();
                framed.extend_from_slice(reply.as_bytes());
                stream.write_all(&framed).expect("reply");
                heard.push(call);
            }
            heard
        })
    }

    #[tokio::test]
    async fn every_call_to_a_socket_with_a_held_key_carries_it_and_no_other_does() {
        let dir = magi_model::scratch::Scratch::new("magi-family-keyed", "one");
        let (ours, theirs) = (dir.join("api@ours.sock"), dir.join("api@theirs.sock"));
        let key = crate::random_hex(32).expect("random");
        hold_key(&ours, &key);
        let (heard_ours, heard_theirs) = (recording(&ours, 1), recording(&theirs, 1));

        Family::dial(&ours)
            .await
            .expect("dial")
            .call("verbs", Vec::new())
            .await
            .expect("verbs");
        blocking::Family::dial(&theirs)
            .expect("dial")
            .call("verbs", Vec::new())
            .expect("verbs");

        let ours = heard_ours.join().expect("ours");
        assert_eq!(ours[0]["key"], json!(key));
        let theirs = heard_theirs.join().expect("theirs");
        assert!(theirs[0].get("key").is_none(), "{}", theirs[0]);
    }

    /// A memory layer started again by another: it answers a write only with `key`, and refuses
    /// any other the way balthasar does. Hands back the key each call carried.
    fn restarted(path: &Path, key: &str) -> std::thread::JoinHandle<Vec<serde_json::Value>> {
        use std::io::{Read, Write};

        let listener = std::os::unix::net::UnixListener::bind(path).expect("bind");
        let key = key.to_owned();
        std::thread::spawn(move || {
            let mut carried = Vec::new();
            let Ok((mut stream, _)) = listener.accept() else {
                return carried;
            };
            for _ in 0..2 {
                let mut head = [0_u8; 4];
                if stream.read_exact(&mut head).is_err() {
                    break;
                }
                let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
                stream.read_exact(&mut body).expect("body");
                let call: serde_json::Value = serde_json::from_slice(&body).expect("json");
                carried.push(call["key"].clone());
                let reply = if call["key"] == json!(key) {
                    json!({ "ok": true, "n": 1, "result": [call["call"].clone()] })
                } else {
                    json!({ "ok": false, "fault": "refused", "error": keys::REFUSED })
                };
                let reply = reply.to_string();
                let mut framed = (reply.len() as u32).to_be_bytes().to_vec();
                framed.extend_from_slice(reply.as_bytes());
                stream.write_all(&framed).expect("reply");
            }
            carried
        })
    }

    #[tokio::test]
    async fn a_write_refused_for_its_key_is_made_again_with_the_key_now_published() {
        use std::io::Write;
        let dir = magi_model::scratch::Scratch::new("magi-family-rekeyed", "one");
        let socket = dir.join("api@again.sock");
        let (old, new) = (
            crate::random_hex(32).expect("random"),
            crate::random_hex(32).expect("random"),
        );
        hold_key(&socket, &old);
        crate::private_file(&crate::key_path(&socket))
            .expect("create")
            .write_all(new.as_bytes())
            .expect("write");
        let served = restarted(&socket, &new);

        let mut open = Family::dial(&socket).await.expect("dial");
        let answered = open.call("observe", Vec::new()).await.expect("observe");
        assert_eq!(answered, vec![json!("observe")]);
        drop(open);
        assert_eq!(served.join().expect("served"), vec![json!(old), json!(new)]);
    }

    /// A caller that gives up on a call leaves its answer in the stream. Read as the next call's,
    /// every answer after it belongs to the call before — a write is told it landed by somebody
    /// else's reply, and the transcript goes missing without anything failing.
    #[tokio::test]
    async fn a_call_given_up_on_does_not_answer_the_next_one() {
        let dir = magi_model::scratch::Scratch::new("magi-family-adrift", "one");
        let path = dir.join("api@one.sock");
        let served = echoing(&path);

        let mut open = Family::dial(&path).await.expect("dial");
        let abandoned = tokio::time::timeout(
            std::time::Duration::from_millis(30),
            open.call("plan", Vec::new()),
        )
        .await;
        assert!(abandoned.is_err(), "the fixture answered too soon to test");

        let answered = open.call("observe", Vec::new()).await.expect("observe");
        assert_eq!(
            answered,
            vec![json!("observe")],
            "the answer to the call before it came back as this one's"
        );
        drop(open);
        let _ = served.join();
    }

    /// A peer answering with the name of the verb it was asked, slowly the first time.
    fn echoing(path: &Path) -> std::thread::JoinHandle<()> {
        use std::io::{Read, Write};

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
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                let reply = json!({ "ok": true, "n": 1, "result": [verb] }).to_string();
                let mut framed = (reply.len() as u32).to_be_bytes().to_vec();
                framed.extend_from_slice(reply.as_bytes());
                let _ = stream.write_all(&framed);
            }
        })
    }
}
