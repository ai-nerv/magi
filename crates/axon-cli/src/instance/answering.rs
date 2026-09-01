//! Answering another instance.
//!
//! One function, and it is pure: a [`Call`] and what this instance knows go in, a [`Reply`]
//! comes out. The socket loop above it does nothing but frame bytes.
//!
//! Split that way because the interesting failures here are not transport failures. "a peer let
//! itself be stopped", "a refusal arrived as a dropped connection", "`n` did not match the
//! result" are all decidable without a socket, and a test that has to bind one to check them is
//! a test nobody writes.

use super::wire::{Call, Message, Reply, VERBS};
use super::{Kind, Reach};
use crate::identity::Identity;

/// What this instance is willing to say about itself.
///
/// Gathered by the caller and handed in, so [`answer`] stays a function of its arguments. The
/// session is behind a lock in the UI thread and a socket handler that reached into it would be
/// holding that lock while a peer decides how fast to read.
#[derive(Debug, Clone)]
pub struct About {
    /// Who this is.
    pub me: Identity,
    /// Whether a turn is running.
    pub busy: bool,
    /// How long it has been running, in seconds.
    pub working_for: u64,
    /// What has arrived and not been read.
    pub inbox: Vec<Message>,
}

/// What a reply asks the caller to do afterwards, beyond sending it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// Nothing.
    Nothing,
    /// Put this in the inbox.
    Keep(Message),
    /// End this instance.
    Stop,
}

/// Answer one call.
///
/// `kind` is how the *caller* reached us — a fork we own asking its parent, or a peer that
/// merely found the socket — and it is the whole of the permission check. Every verb that
/// changes anything goes through [`Reach::allows`] before it does.
#[must_use]
pub fn answer(call: &Call, about: &About, kind: Kind) -> (Reply, Then) {
    match call.call.as_str() {
        // From the first version, because it cannot be added quietly later: a family where one
        // tool can be asked what it speaks and another cannot has stopped being one.
        "verbs" => (
            Reply::of(serde_json::json!(
                VERBS
                    .iter()
                    .map(|(name, said)| serde_json::json!({"verb": name, "does": said}))
                    .collect::<Vec<_>>()
            )),
            Then::Nothing,
        ),
        "identity" => (
            Reply::of(serde_json::json!({
                "project": about.me.project,
                "role": about.me.role,
                "id": about.me.id,
                "full": about.me.full(),
            })),
            Then::Nothing,
        ),
        "status" => (
            Reply::of(serde_json::json!({
                "busy": about.busy,
                "working_for": about.working_for,
                "waiting": about.inbox.len(),
            })),
            Then::Nothing,
        ),
        "inbox" => (Reply::of(serde_json::json!(about.inbox)), Then::Nothing),
        "tell" => {
            if !Reach::Tell.allows(kind) {
                return (Reply::refused(spurned(Reach::Tell)), Then::Nothing);
            }
            let (Some(from), Some(text)) = (text_at(call, 0), text_at(call, 1)) else {
                return (
                    Reply::refused("tell takes who is speaking and what they said"),
                    Then::Nothing,
                );
            };
            (Reply::done(), Then::Keep(Message::new(&from, &text)))
        }
        "stop" => {
            if !Reach::Stop.allows(kind) {
                return (Reply::refused(spurned(Reach::Stop)), Then::Nothing);
            }
            (Reply::done(), Then::Stop)
        }
        other => (
            Reply::refused(format!("no such call: {other}")),
            Then::Nothing,
        ),
    }
}

/// A string argument, if there is one there.
fn text_at(call: &Call, at: usize) -> Option<String> {
    call.args.get(at)?.as_str().map(ToOwned::to_owned)
}

/// Why a peer was turned down.
fn spurned(reach: Reach) -> String {
    let verb = match reach {
        Reach::Ask => "ask",
        Reach::Tell => "tell",
        Reach::Stop => "stop",
    };
    format!("only a fork may be {verb}ped: this connection is a peer's")
}

/// A peer may ask and nothing else, and every answer is the shape the family agreed.
#[cfg(test)]
mod tests {
    use super::*;

    fn about() -> About {
        About {
            me: Identity {
                project: "axon".to_owned(),
                role: "main".to_owned(),
                id: "alpha".to_owned(),
            },
            busy: false,
            working_for: 0,
            inbox: Vec::new(),
        }
    }

    fn call(name: &str, args: &[&str]) -> Call {
        Call {
            call: name.to_owned(),
            args: args.iter().map(|a| serde_json::json!(a)).collect(),
        }
    }

    #[test]
    fn a_peer_may_ask() {
        for verb in ["verbs", "identity", "status", "inbox"] {
            let (reply, then) = answer(&call(verb, &[]), &about(), Kind::Peer);
            assert!(reply.ok, "{verb} was refused to a peer");
            assert_eq!(then, Then::Nothing, "{verb} asked for something to happen");
        }
    }

    #[test]
    fn a_peer_may_not_stop_us() {
        // The one that matters. A session any process knowing its name can end is a session
        // somebody loses while they are typing into it.
        let (reply, then) = answer(&call("stop", &[]), &about(), Kind::Peer);
        assert!(!reply.ok);
        assert_eq!(then, Then::Nothing, "it was going to stop anyway");
        assert!(reply.error.unwrap_or_default().contains("fork"));
    }

    #[test]
    fn a_peer_may_put_things_in_our_inbox() {
        // An instance nobody forked is still one you can talk to. What arrives is a message,
        // not an instruction: it waits in the inbox and this session decides.
        let (reply, then) = answer(
            &call("tell", &["axon/main/beta", "do this"]),
            &about(),
            Kind::Peer,
        );
        assert!(reply.ok, "{:?}", reply.error);
        let Then::Keep(message) = then else {
            panic!("nothing was kept");
        };
        assert_eq!(message.from, "axon/main/beta");
    }

    #[test]
    fn a_fork_may_do_all_of_it() {
        let (reply, then) = answer(&call("stop", &[]), &about(), Kind::Fork);
        assert!(reply.ok);
        assert_eq!(then, Then::Stop);

        let (reply, then) = answer(
            &call("tell", &["axon/main/gamma", "found it"]),
            &about(),
            Kind::Fork,
        );
        assert!(reply.ok);
        let Then::Keep(message) = then else {
            panic!("nothing was kept");
        };
        assert_eq!(message.from, "axon/main/gamma");
        assert_eq!(message.text, "found it");
    }

    #[test]
    fn an_unknown_verb_is_refused_by_name() {
        // A reply, not a dropped connection: "no such call: nope" says what to fix and
        // "connection reset" does not.
        let (reply, _) = answer(&call("nope", &[]), &about(), Kind::Fork);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_deref(), Some("no such call: nope"));
    }

    #[test]
    fn tell_without_its_arguments_says_what_it_wanted() {
        let (reply, then) = answer(&call("tell", &[]), &about(), Kind::Fork);
        assert!(!reply.ok);
        assert_eq!(then, Then::Nothing);
        assert!(reply.error.unwrap_or_default().contains("said"));
    }

    #[test]
    fn every_answer_keeps_the_family_shape() {
        // `n` is the length of `result`, whatever happened. A sibling that unpacks a list reads
        // a mismatch as nothing at all, and nothing at all looks like an empty session.
        for verb in ["verbs", "identity", "status", "inbox", "stop", "nope"] {
            let (reply, _) = answer(&call(verb, &[]), &about(), Kind::Fork);
            assert_eq!(reply.n, reply.result.len(), "{verb}");
        }
    }

    #[test]
    fn every_verb_it_advertises_is_one_it_answers() {
        // The list and the dispatch drift the moment either is edited, and a `verbs()` that
        // lies is worse than none: it is the one call a stranger trusts.
        for (verb, _) in VERBS {
            let (reply, _) = answer(&call(verb, &["a", "b"]), &about(), Kind::Fork);
            assert!(
                reply.error.as_deref() != Some(&format!("no such call: {verb}")),
                "{verb} is advertised and not answered"
            );
        }
    }

    #[test]
    fn status_says_what_is_waiting() {
        let mut about = about();
        about.busy = true;
        about.working_for = 12;
        about.inbox.push(Message::new("axon/main/beta", "hello"));
        let (reply, _) = answer(&call("status", &[]), &about, Kind::Peer);
        let said = &reply.result[0];
        assert_eq!(said["busy"], true);
        assert_eq!(said["working_for"], 12);
        assert_eq!(said["waiting"], 1);
    }
}
