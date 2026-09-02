//! Answering another instance.
//!
//! One function, and it is pure: a [`Call`], what this instance knows, and who worked out to be
//! calling go in, a [`Reply`] comes out. The socket loop above it frames bytes and reads the
//! caller's place in the tree off the directory; it decides nothing.
//!
//! Split that way because the interesting failures here are not transport failures. "a cousin
//! was let through", "a stop was honoured without the secret", "a refusal arrived as a dropped
//! connection", "`n` did not match the result" are all decidable without a socket, and a test
//! that has to bind one to check them is a test nobody writes.

use super::Reach;
use super::policy::{self, Whom};
use super::wire::{Call, Message, Reply, VERBS};
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
    /// Who started it, or `None` if it is a main.
    pub parent: Option<String>,
    /// The secret it was started with, which a `stop` has to quote back.
    pub token: Option<String>,
    /// Whether a turn is running.
    pub busy: bool,
    /// How long it has been running, in seconds.
    pub working_for: u64,
    /// What has arrived and not been read.
    pub inbox: Vec<Message>,
}

impl About {
    /// This session's place in the tree.
    #[must_use]
    pub fn whom(&self) -> Whom {
        Whom {
            project: self.me.project.clone(),
            id: self.me.id.clone(),
            parent: self.parent.clone(),
        }
    }
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
/// `caller` is who the far end worked out to be, read off the project directory rather than
/// taken from the frame — so a session cannot describe its own place in the tree. `None` means
/// it did not say who it was, and then only `verbs` is answered: everything else is about this
/// session, and a stranger has no standing to ask.
#[must_use]
pub fn answer(call: &Call, about: &About, caller: Option<&Whom>) -> (Reply, Then) {
    // From the first version and before any permission check, because it cannot be added
    // quietly later: a family where one tool can be asked what it speaks and another cannot has
    // stopped being one, and a client that has to guess the vocabulary guesses wrong first.
    if call.call == "verbs" {
        return (
            Reply::of(serde_json::json!(
                VERBS
                    .iter()
                    .map(|(name, said)| serde_json::json!({"verb": name, "does": said}))
                    .collect::<Vec<_>>()
            )),
            Then::Nothing,
        );
    }
    let Some(caller) = caller else {
        return (
            Reply::refused("say who is calling: every call but `verbs` needs a `from`"),
            Then::Nothing,
        );
    };
    let me = about.whom();
    // Two relations, because the question has a direction. `relation` is how the caller stands
    // to this session, which is what `kin` reports; `theirs` is how this session stands to the
    // caller, which is what decides whether they may. Asking the first one would have let a
    // child stop its parent — the check reads the same from both ends and the answer does not.
    let relation = policy::between(&me, caller);
    let theirs = policy::between(caller, &me);
    let wanted = match call.call.as_str() {
        "identity" | "kin" | "status" | "inbox" => Reach::Ask,
        "tell" => Reach::Tell,
        "stop" => Reach::Stop,
        other => {
            return (
                Reply::refused(format!("no such call: {other}")),
                Then::Nothing,
            );
        }
    };
    if !policy::may(caller, theirs, wanted) {
        return (
            Reply::refused(policy::refusal(caller, theirs, wanted)),
            Then::Nothing,
        );
    }
    match call.call.as_str() {
        "identity" => (
            Reply::of(serde_json::json!({
                "project": about.me.project,
                "role": about.me.role,
                "id": about.me.id,
                "full": about.me.full(),
                "parent": about.parent,
                "main": about.parent.is_none(),
            })),
            Then::Nothing,
        ),
        // Said out loud so a caller does not have to work it out from the directory a second
        // time, and so the two answers can be compared when they disagree.
        "kin" => (
            Reply::of(serde_json::json!({
                "relation": relation.word(),
                "means": relation.named(),
                "may_stop": policy::may(caller, theirs, Reach::Stop),
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
            let Some(text) = text_at(call, 0) else {
                return (Reply::refused("tell takes what to say"), Then::Nothing);
            };
            let sort = text_at(call, 1)
                .and_then(|name| super::wire::Sort::read(&name))
                .unwrap_or_default();
            let about_what = text_at(call, 2);
            // Project and id from the connection, never from an argument: a message that could
            // name its own sender is a message anybody can forge into anybody's inbox.
            //
            // The role is the exception, and only because it is not worth taking: it is what a
            // session says it is *for*, it grants nothing, and the alternative is stamping every
            // message `main` and telling the reader something untrue about who wrote it.
            let role = Identity::read(call.from.as_deref().unwrap_or_default())
                .map_or_else(|| "main".to_owned(), |claimed| claimed.role);
            let from = Identity {
                project: caller.project.clone(),
                role,
                id: caller.id.clone(),
            }
            .full();
            (
                Reply::done(),
                Then::Keep(Message::sent(&from, &text, sort, about_what)),
            )
        }
        "stop" => {
            // The relation said the caller is the one that started this session. The secret says
            // it is actually them: a name is free to claim and this is not.
            let Some(mine) = about.token.as_deref() else {
                return (
                    Reply::refused(
                        "this session was not started by another, so nothing may stop it",
                    ),
                    Then::Nothing,
                );
            };
            if call.token.as_deref() != Some(mine) {
                return (
                    Reply::refused("that is not the secret this session was started with"),
                    Then::Nothing,
                );
            }
            (Reply::done(), Then::Stop)
        }
        _ => unreachable!("the vocabulary was matched above"),
    }
}

/// A string argument, if there is one there.
fn text_at(call: &Call, at: usize) -> Option<String> {
    call.args.get(at)?.as_str().map(ToOwned::to_owned)
}

/// The walls hold over a call, and every answer is the shape the family agreed.
#[cfg(test)]
mod tests {
    use super::*;

    fn about() -> About {
        About {
            me: Identity {
                project: "axon".to_owned(),
                role: "main".to_owned(),
                id: "alpha-rho".to_owned(),
            },
            parent: None,
            token: None,
            busy: false,
            working_for: 0,
            inbox: Vec::new(),
        }
    }

    fn whom(project: &str, id: &str, parent: Option<&str>) -> Whom {
        Whom {
            project: project.to_owned(),
            id: id.to_owned(),
            parent: parent.map(ToOwned::to_owned),
        }
    }

    fn call(verb: &str) -> Call {
        Call {
            call: verb.to_owned(),
            ..Call::default()
        }
    }

    #[test]
    fn verbs_is_answered_before_anybody_has_said_who_they_are() {
        // A client that has to guess the vocabulary guesses wrong first.
        let (reply, then) = answer(&call("verbs"), &about(), None);
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.n, reply.result.len());
        assert_eq!(then, Then::Nothing);
    }

    #[test]
    fn everything_else_needs_a_caller() {
        for verb in ["identity", "status", "inbox", "tell", "stop"] {
            let (reply, then) = answer(&call(verb), &about(), None);
            assert!(!reply.ok, "{verb} answered a stranger");
            assert_eq!(then, Then::Nothing);
        }
    }

    #[test]
    fn a_main_answers_another_main_in_the_same_project() {
        let them = whom("axon", "beta-nu", None);
        let (reply, _) = answer(&call("status"), &about(), Some(&them));
        assert!(reply.ok, "{reply:?}");
    }

    #[test]
    fn nothing_from_another_project_is_answered() {
        // The project wall, over a call rather than over the directory.
        let them = whom("other", "beta-nu", None);
        for verb in ["identity", "status", "inbox", "tell", "stop"] {
            let (reply, _) = answer(&call(verb), &about(), Some(&them));
            assert!(!reply.ok, "{verb} crossed the wall");
            let why = reply.error.unwrap_or_default();
            assert!(why.contains("projects"), "{why}");
        }
    }

    #[test]
    fn a_cousin_is_refused_at_the_default_and_told_what_would_help() {
        let mut about = about();
        about.parent = Some("beta-nu".to_owned());
        let them = whom("axon", "tau-chi", Some("gamma-xi"));
        let (reply, _) = answer(&call("status"), &about, Some(&them));
        assert!(!reply.ok);
        let why = reply.error.unwrap_or_default();
        assert!(why.contains("agent_talk"), "{why}");
    }

    #[test]
    fn a_message_is_stamped_with_who_actually_sent_it() {
        // Never with an argument. A message that could name its own sender is one anybody can
        // forge into anybody's inbox.
        let them = whom("axon", "beta-nu", None);
        let call = Call {
            call: "tell".to_owned(),
            args: vec![serde_json::json!("the parser is done")],
            from: Some("axon/main/somebody-else".to_owned()),
            token: None,
        };
        let (reply, then) = answer(&call, &about(), Some(&them));
        assert!(reply.ok, "{reply:?}");
        let Then::Keep(message) = then else {
            panic!("it was not kept: {then:?}");
        };
        assert_eq!(message.from, "axon/main/beta-nu");
    }

    #[test]
    fn a_message_carries_the_sort_it_was_sent_as() {
        let them = whom("axon", "beta-nu", None);
        let call = Call {
            call: "tell".to_owned(),
            args: vec![
                serde_json::json!("I am stuck"),
                serde_json::json!("trouble"),
            ],
            ..Call::default()
        };
        let (_, then) = answer(&call, &about(), Some(&them));
        let Then::Keep(message) = then else {
            panic!("it was not kept");
        };
        assert!(message.sort.interrupts(), "{:?}", message.sort);
    }

    #[test]
    fn a_main_nobody_started_cannot_be_stopped_by_anything() {
        // It holds no secret, so there is nothing to quote back.
        let them = whom("axon", "beta-nu", None);
        let (reply, then) = answer(&call("stop"), &about(), Some(&them));
        assert!(!reply.ok);
        assert_eq!(then, Then::Nothing);
    }

    #[test]
    fn a_parent_that_knows_the_secret_may_stop_its_child() {
        let mut about = about();
        about.parent = Some("beta-nu".to_owned());
        about.token = Some("s3cret".to_owned());
        let parent = whom("axon", "beta-nu", None);
        let call = Call {
            call: "stop".to_owned(),
            token: Some("s3cret".to_owned()),
            ..Call::default()
        };
        let (reply, then) = answer(&call, &about, Some(&parent));
        assert!(reply.ok, "{reply:?}");
        assert_eq!(then, Then::Stop);
    }

    #[test]
    fn the_name_alone_is_not_enough_to_stop_anything() {
        // The whole reason there is a secret. Any process of this user can connect claiming to
        // be the parent, and a session somebody loses while typing into it is the cost.
        let mut about = about();
        about.parent = Some("beta-nu".to_owned());
        about.token = Some("s3cret".to_owned());
        let pretending = whom("axon", "beta-nu", None);
        for token in [None, Some("guessed".to_owned())] {
            let call = Call {
                call: "stop".to_owned(),
                token,
                ..Call::default()
            };
            let (reply, then) = answer(&call, &about, Some(&pretending));
            assert!(!reply.ok, "a stop went through without the secret");
            assert_eq!(then, Then::Nothing);
        }
    }

    #[test]
    fn a_sibling_holding_the_secret_still_may_not_stop_it() {
        // The secret is proof of identity, not a permission. Both checks stand.
        let mut about = about();
        about.parent = Some("beta-nu".to_owned());
        about.token = Some("s3cret".to_owned());
        let sibling = whom("axon", "zeta-pi", Some("beta-nu"));
        let call = Call {
            call: "stop".to_owned(),
            token: Some("s3cret".to_owned()),
            ..Call::default()
        };
        let (reply, then) = answer(&call, &about, Some(&sibling));
        assert!(!reply.ok, "{reply:?}");
        assert_eq!(then, Then::Nothing);
    }

    #[test]
    fn kin_says_how_the_caller_stands_and_whether_it_may_stop() {
        let mut about = about();
        about.parent = Some("beta-nu".to_owned());
        let parent = whom("axon", "beta-nu", None);
        let (reply, _) = answer(&call("kin"), &about, Some(&parent));
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.result[0]["relation"], "parent");
        assert_eq!(reply.result[0]["may_stop"], true);
    }

    #[test]
    fn a_verb_nothing_answers_says_so_rather_than_dropping_the_connection() {
        let them = whom("axon", "beta-nu", None);
        let (reply, _) = answer(&call("nope"), &about(), Some(&them));
        assert!(!reply.ok);
        assert!(
            reply.error.unwrap_or_default().contains("nope"),
            "it did not say which"
        );
    }

    #[test]
    fn every_verb_the_family_is_told_about_is_one_that_answers() {
        // `verbs` promising something that refuses everything is worse than not listing it.
        let mut about = about();
        about.parent = Some("beta-nu".to_owned());
        about.token = Some("s3cret".to_owned());
        let parent = whom("axon", "beta-nu", None);
        for (name, _) in VERBS {
            let call = Call {
                call: (*name).to_owned(),
                args: vec![serde_json::json!("something")],
                token: Some("s3cret".to_owned()),
                from: None,
            };
            let (reply, _) = answer(&call, &about, Some(&parent));
            assert!(reply.ok, "{name} is listed and refuses: {reply:?}");
        }
    }
}
