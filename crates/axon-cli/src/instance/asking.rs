//! Reaching another instance.
//!
//! The other half of [`super::serving`], and deliberately blocking. The caller is a tool peer
//! whose whole existence is one round trip — it has no UI to keep responsive and no turn to
//! yield to — and a blocking socket there is a dozen lines where an async one would be a
//! runtime, a spawn and a channel to carry the answer back out of it.
//!
//! # One connection, several calls
//!
//! [`Held`] stays open until it is dropped. Closing after each call is the tempting
//! simplification and it is the one the family's own guidance warns about: a client that holds a
//! connection is the obvious way to write one, and it dies on its *second* call with a broken
//! pipe. `list`, `status` on each of them, then `send` to one is four calls and one connection.
//!
//! # Every call says who is making it
//!
//! Not as courtesy — [`super::answering`] refuses anything but `verbs` without it, because
//! every other verb is about that session and a stranger has no standing to ask. The name is
//! taken at face value; what it buys is a *relation*, which is read off the directory at the
//! far end and is not the caller's to claim.

use super::framing;
use super::wire::{Call, Reply};
use crate::identity::Identity;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long to wait for a connection, and then for each answer.
///
/// A session mid-turn answers its socket from another task, so this is not "how long a turn
/// takes" — it is how long a *healthy* peer can take to notice a frame. Long enough to survive
/// a loaded machine, short enough that a wedged instance does not hold a tool call open until
/// the model gives up on it.
const PATIENCE: Duration = Duration::from_secs(10);

/// An open connection to one instance.
pub struct Held {
    stream: UnixStream,
    /// Who this session is, put on every call.
    me: String,
}

impl Held {
    /// Open a connection to whatever is listening at `path`.
    pub fn at(path: &Path, me: &Identity) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        // Both directions: a peer that accepted and then never answered would otherwise hold
        // this open for as long as it felt like, and the tool call with it.
        stream.set_read_timeout(Some(PATIENCE))?;
        stream.set_write_timeout(Some(PATIENCE))?;
        Ok(Self {
            stream,
            me: me.full(),
        })
    }

    /// Open a connection to a session by name, in this project.
    pub fn to(them: &Identity, me: &Identity) -> std::io::Result<Self> {
        Self::at(&super::listening_at(them), me)
    }

    /// Make one call and read its answer.
    ///
    /// A refusal comes back as a [`Reply`] with `ok: false`, not as an error: that is the
    /// family's shape, and it is the difference between "no such call: nope", which says what
    /// to fix, and "connection reset", which does not.
    pub fn call(&mut self, verb: &str, args: Vec<serde_json::Value>) -> std::io::Result<Reply> {
        self.ask(Call {
            call: verb.to_owned(),
            args,
            from: Some(self.me.clone()),
            token: None,
        })
    }

    /// The same, carrying the secret the far end was started with.
    ///
    /// Only `stop` needs one. Kept separate rather than an `Option` on every call so a verb
    /// cannot pick up a secret by accident, and so the one place a secret is sent is one line
    /// that can be read.
    pub fn call_with(
        &mut self,
        verb: &str,
        args: Vec<serde_json::Value>,
        token: &str,
    ) -> std::io::Result<Reply> {
        self.ask(Call {
            call: verb.to_owned(),
            args,
            from: Some(self.me.clone()),
            token: Some(token.to_owned()),
        })
    }

    /// Write one call, read one reply.
    fn ask(&mut self, call: Call) -> std::io::Result<Reply> {
        framing::write_to(&mut Writing(&self.stream), &call)?;
        framing::read_from(&mut Reading(&self.stream))
    }
}

/// A `&UnixStream` writes; the borrow is what lets one connection do both halves.
struct Writing<'a>(&'a UnixStream);

impl Write for Writing<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        (&*self.0).write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        (&*self.0).flush()
    }
}

/// And reads.
struct Reading<'a>(&'a UnixStream);

impl Read for Reading<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&*self.0).read(buf)
    }
}

/// Whether anything is actually listening as `them`.
///
/// A socket file outlives the process that made it, so the directory says who *was* here. This
/// is the cheapest question that distinguishes a running session from a crash's leftovers, and
/// it is worth asking before a message is reported as delivered.
#[must_use]
pub fn answers(them: &Identity, me: &Identity) -> bool {
    Held::to(them, me).is_ok()
}

/// A refusal is a reply, and a caller always says who it is.
#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Identity {
        Identity {
            project: "axon".to_owned(),
            id: "alpha-rho".to_owned(),
        }
    }

    #[test]
    fn a_call_carries_the_caller_s_name() {
        // Without it the far end answers `verbs` and refuses everything else, which presents as
        // "that instance does not work" rather than as a client that forgot to introduce itself.
        let held = Held {
            // Any fd will do: nothing is written, and building the frame is what is under test.
            stream: UnixStream::pair().expect("a pair").0,
            me: me().full(),
        };
        let call = Call {
            call: "status".to_owned(),
            args: Vec::new(),
            from: Some(held.me.clone()),
            token: None,
        };
        assert_eq!(call.from.as_deref(), Some("axon/alpha-rho"));
        assert!(call.token.is_none(), "an ordinary call carries no secret");
    }

    #[test]
    fn a_round_trip_over_a_real_socket_pair_reads_back() {
        // The framing and the two halves of one stream, over something a kernel made.
        let (mine, theirs) = UnixStream::pair().expect("a pair");
        let mut held = Held {
            stream: mine,
            me: me().full(),
        };
        let answering = std::thread::spawn(move || {
            let call: Call = framing::read_from(&mut Reading(&theirs)).expect("reads");
            assert_eq!(call.call, "status");
            assert_eq!(call.from.as_deref(), Some("axon/alpha-rho"));
            framing::write_to(
                &mut Writing(&theirs),
                &Reply::of(serde_json::json!({"busy": true})),
            )
            .expect("writes");
        });
        let reply = held.call("status", Vec::new()).expect("answered");
        answering.join().expect("the far end finished");
        assert!(reply.ok);
        assert_eq!(reply.result[0]["busy"], true);
    }

    #[test]
    fn nothing_listening_is_an_error_rather_than_a_wait() {
        let missing = Identity {
            project: "no-such-project-here".to_owned(),
            id: "nobody-nowhere".to_owned(),
        };
        assert!(!answers(&missing, &me()));
    }
}
