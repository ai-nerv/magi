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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Call {
    /// Which verb.
    pub call: String,
    /// Its arguments, in order.
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
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
    ("identity", "its project, role and id"),
    ("status", "whether it is working, and for how long"),
    ("inbox", "messages it has been sent and not yet read"),
    ("tell", "put a message in its inbox"),
    ("stop", "end it — a fork only, never a peer"),
];

/// A message from one instance to another.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    /// Who sent it, as `project/role/id`.
    pub from: String,
    /// What they said.
    pub text: String,
    /// When, in milliseconds since the epoch.
    pub at: u64,
}

impl Message {
    /// A message from `from` saying `text`, stamped now.
    #[must_use]
    pub fn new(from: &str, text: &str) -> Self {
        Self {
            from: from.to_owned(),
            text: text.to_owned(),
            at: u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_millis()),
            )
            .unwrap_or(0),
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
        let message = Message::new("axon/main/alpha", "stop what you are doing");
        assert_eq!(message.from, "axon/main/alpha");
        assert!(message.at > 0, "and when it was sent");
    }
}
