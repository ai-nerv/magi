//! What one instance says to another.
//!
//! **Mirrored on purpose.** Every axon both listens and dials, and both halves are these types:
//! a main agent asking a fork what it is doing, and the fork asking back, are the same frames
//! travelling the other way. There is no supervisor vocabulary and no worker vocabulary,
//! because the moment there were two an agent could be one but not the other, and a subagent
//! that cannot ask its parent a question is a subagent that has to guess.
//!
//! # The shape is the family's, not axon's
//!
//! ```text
//! -> {"call":"status","args":[]}
//! <- {"ok":true,"n":1,"result":[{"busy":false,…}]}
//! ```
//!
//! Four-byte big-endian length, then the body. `result` is a **list** and `n` says how long it
//! is, which matters more than it looks: hexe, oslo and aeon answer in exactly this shape, and a
//! sibling that unpacks a list would read a bare value as *nothing at all*. `session()` would
//! come back empty rather than wrong, and an empty answer looks like an empty session — so the
//! bug presents as "that peer has no state" for as long as it takes somebody to put `socat` on
//! the socket. It is settled here before either side ships.
//!
//! A refused call is a **reply**, not a dropped connection: `{"ok":false,"error":…}`. The caller
//! then sees axon's error rather than a transport error, and "no such call: nope" says what to
//! fix where "connection reset" does not.

use serde::{Deserialize, Serialize};

/// One call, as it arrives.
///
/// `call` and `args` are the family shape and nothing else belongs in them. `from` and `token`
/// are axon`s own, and both are optional so a sibling tool poking the socket with `socat` still
/// gets an answer to `verbs` rather than a parse error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Call {
    /// Which verb.
    pub call: String,
    /// Its arguments, in order.
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
    /// Who is calling, as `project/id`.
    ///
    /// Taken at face value for everything but `stop`. It has to be: this is one user talking to
    /// itself in one directory, and a check that cannot be enforced reads like security to
    /// whoever comes along next. What it buys is a *relation* — the answer to "may I" is worked
    /// out from the directory, not from anything else in this frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The secret handed down at spawn, for the one verb that needs proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// One reply, as it goes back.
///
/// Built through [`Reply::of`] and [`Reply::refused`] rather than by hand, so the `n`/`result`
/// invariant holds in one place instead of at every call site that answers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reply {
    /// Whether the call was answered.
    pub ok: bool,
    /// How many values came back. Always `result.len()`.
    #[serde(default)]
    pub n: usize,
    /// The values, in order.
    #[serde(default)]
    pub result: Vec<serde_json::Value>,
    /// Why not, when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    /// An answer of one value.
    #[must_use]
    pub fn of(value: serde_json::Value) -> Self {
        Self {
            ok: true,
            n: 1,
            result: vec![value],
            error: None,
        }
    }

    /// An answer of none, for a verb that does something rather than reporting something.
    #[must_use]
    pub fn done() -> Self {
        Self {
            ok: true,
            n: 0,
            result: Vec::new(),
            error: None,
        }
    }

    /// A refusal, which is still a reply.
    #[must_use]
    pub fn refused(why: impl Into<String>) -> Self {
        Self {
            ok: false,
            n: 0,
            result: Vec::new(),
            error: Some(why.into()),
        }
    }
}

/// The verbs an instance answers.
///
/// A named, small subset, and deliberately not a mirror of everything axon can do. Most of what
/// a session knows is meaningless to a peer and some of it is dangerous — anything that runs a
/// command is a remote shell wearing a friendly name, and none of that is in the first cut.
///
/// `verbs` is here from the first version because it cannot be added quietly later: a family
/// where one tool can be asked what it speaks and another cannot has stopped being a family.
pub const VERBS: &[(&str, &str)] = &[
    ("verbs", "what this instance answers"),
    ("identity", "its project, id and who started it"),
    (
        "kin",
        "how the caller stands to it: parent, child, sibling, main, cousin",
    ),
    (
        "status",
        "whether it is working, for how long, and what is waiting",
    ),
    ("inbox", "messages it has been sent and not yet acted on"),
    ("tell", "put a message of any sort in its inbox"),
    (
        "stop",
        "end it — only from the session that started it, with the secret it was given",
    ),
];

/// What a message is for.
///
/// One inbox, sorted by what each thing is, rather than a channel per kind. A worker that has
/// to poll five queues to find out what is happening will poll four of them and miss the fifth,
/// and the one it misses is always the urgent one.
///
/// The sort is what makes a surface out of a pipe: `attention` and `note` travel identically and
/// mean entirely different things to whoever reads them, and only the sender knows which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    /// Something worth knowing, wanting nothing back.
    #[default]
    Note,
    /// A question, which expects an [`Sort::Answer`] quoting its id.
    Question,
    /// An answer to a question, with `about` naming it.
    Answer,
    /// "I need you." The one that is allowed to interrupt.
    Attention,
    /// "I am taking this piece of work."
    Claim,
    /// "I am done with it, or I never started."
    Release,
    /// "This is yours now" — a piece of work moved, not copied.
    Handoff,
    /// Something is wrong and the sender cannot go on.
    Trouble,
}

impl Sort {
    /// Read what somebody wrote, or `None` if it is not one of these.
    #[must_use]
    pub fn read(name: &str) -> Option<Self> {
        Some(match name {
            "note" => Self::Note,
            "question" => Self::Question,
            "answer" => Self::Answer,
            "attention" => Self::Attention,
            "claim" => Self::Claim,
            "release" => Self::Release,
            "handoff" => Self::Handoff,
            "trouble" => Self::Trouble,
            _ => return None,
        })
    }

    /// Whether this is meant to reach somebody who is mid-turn.
    ///
    /// Only two things are: being asked for help, and being told something has gone wrong.
    /// Everything else waits, because an inbox that interrupts for every note is an inbox
    /// nobody leaves switched on.
    #[must_use]
    pub fn interrupts(self) -> bool {
        matches!(self, Self::Attention | Self::Trouble)
    }

    /// Whether the sender is waiting for something back.
    #[must_use]
    pub fn expects_an_answer(self) -> bool {
        matches!(self, Self::Question)
    }
}

/// A message from one instance to another.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    /// This message, so an answer can quote it.
    pub id: String,
    /// Who sent it, as `project/id`.
    pub from: String,
    /// What kind of thing it is.
    #[serde(default)]
    pub sort: Sort,
    /// What they said.
    pub text: String,
    /// The message this one is about, for an answer or a release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    /// When, in milliseconds since the epoch.
    pub at: u64,
}

impl Message {
    /// A plain note from `from`.
    ///
    /// Only tests build one this way. Everything real arrives over the socket, where the sort is
    /// part of what was sent and the sender is worked out rather than given.
    #[cfg(test)]
    #[must_use]
    pub fn new(from: &str, text: &str) -> Self {
        Self::sent(from, text, Sort::Note, None)
    }

    /// A message of any sort, stamped now.
    #[must_use]
    pub fn sent(from: &str, text: &str, sort: Sort, about: Option<String>) -> Self {
        let at = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis()),
        )
        .unwrap_or(0);
        Self {
            // The clock plus the sender, which is unique enough for something two processes
            // exchange and short enough for a model to quote back without mistyping it.
            id: format!("{}-{at:x}", &from.replace('/', "-")),
            from: from.to_owned(),
            sort,
            text: text.to_owned(),
            about,
            at,
        }
    }
}

/// The shape is the family's, and a refusal is a reply.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_carries_a_list_and_says_how_long_it_is() {
        // The thing that fails silently between siblings. A tool answering `{"result": value}`
        // and one answering `{"result":[value],"n":1}` read each other as having returned
        // nothing, and an empty answer looks like an empty session rather than an error.
        let reply = Reply::of(serde_json::json!({"busy": false}));
        let json = serde_json::to_value(&reply).expect("encodes");
        assert!(json["result"].is_array(), "result must be a list: {json}");
        assert_eq!(json["n"], 1);
    }

    #[test]
    fn n_is_always_the_length_of_the_result() {
        for reply in [
            Reply::of(serde_json::json!("one")),
            Reply::done(),
            Reply::refused("no such call: nope"),
        ] {
            assert_eq!(reply.n, reply.result.len(), "{reply:?}");
        }
    }

    #[test]
    fn a_refusal_is_a_reply_and_says_why() {
        let reply = Reply::refused("no such call: nope");
        assert!(!reply.ok);
        assert_eq!(reply.error.as_deref(), Some("no such call: nope"));
        let json = serde_json::to_value(&reply).expect("encodes");
        assert_eq!(json["ok"], false);
    }

    #[test]
    fn a_successful_reply_carries_no_error_field_at_all() {
        // Rather than a null one: a client checking `error ~= nil` is the obvious way to write
        // one, and a null that serialises would make every success look like a failure.
        let json = serde_json::to_value(Reply::done()).expect("encodes");
        assert!(json.get("error").is_none(), "{json}");
    }

    #[test]
    fn a_call_with_no_arguments_still_reads() {
        // The wire omits an empty list, and a verb like `status` takes none.
        let call: Call = serde_json::from_str(r#"{"call":"status"}"#).expect("reads");
        assert_eq!(call.call, "status");
        assert!(call.args.is_empty());
    }

    #[test]
    fn a_reply_survives_the_round_trip() {
        let sent = Reply::of(serde_json::json!({"id": "gamma"}));
        let text = serde_json::to_string(&sent).expect("encodes");
        let back: Reply = serde_json::from_str(&text).expect("decodes");
        assert_eq!(sent, back);
    }

    #[test]
    fn every_verb_is_named_once_and_described() {
        let mut names: Vec<&str> = VERBS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let held = names.len();
        names.dedup();
        assert_eq!(names.len(), held, "a verb is listed twice");
        assert!(names.contains(&"verbs"), "it must say what it speaks");
        for (name, said) in VERBS {
            assert!(!said.is_empty(), "{name} has no description");
        }
    }

    #[test]
    fn nothing_that_runs_a_command_is_in_the_first_cut() {
        // A socket that runs commands is remote code execution with a friendly name.
        for (name, _) in VERBS {
            assert!(
                !["run", "shell", "exec", "eval", "tool"].contains(name),
                "{name} does not belong on a socket"
            );
        }
    }

    #[test]
    fn a_message_remembers_who_sent_it() {
        // A message with no sender cannot be replied to, which is the whole point of mirroring.
        let message = Message::new("axon/alpha-rho", "stop what you are doing");
        assert_eq!(message.from, "axon/alpha-rho");
        assert!(message.at > 0, "and when it was sent");
    }
}
