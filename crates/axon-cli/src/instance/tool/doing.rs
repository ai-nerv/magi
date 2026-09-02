//! Working out what a verb means, and then doing it.
//!
//! Split from [`super`] because the two halves fail differently. Deciding is pure — a name that
//! does not parse, a verb that needs a `message`, a wall this session is on the wrong side of —
//! and every one of those is worth a test that binds nothing. Doing it is a socket, and what
//! goes wrong there is that nobody is listening.
//!
//! The order matters. Everything decidable is decided *before* anything is dialled, so a model
//! that asked for something it may not have gets told what it may do instead of spending the
//! round trip finding out. The far end checks again — a caller is not to be trusted with its own
//! permissions — but by then the turn has already been paid for.

use super::{SPEAKS, Standing, TOOL, VERBS};
use crate::instance::policy;
use crate::instance::wire::{Reply, Sort};
use crate::instance::{Address, Reach, asking};
use axon_tools::Output;
use serde_json::Value;

/// A call this tool decided to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// Which verb the model asked for.
    pub verb: String,
    /// Which instance, resolved against this session.
    pub who: crate::identity::Identity,
    /// What kind of message it carries, for the verbs that carry one.
    pub sort: Sort,
    /// What to say, for the verbs that say something.
    pub message: Option<String>,
    /// The message being answered or released, for the verbs that quote one.
    pub about: Option<String>,
    /// The secret the far end was started with, for the one verb that has to prove itself.
    pub token: Option<String>,
}

/// What sort of message a verb sends.
///
/// The verb *is* the sort, for everything but `send` — which takes one, because "put this in
/// their inbox" is the general case and the others are it with a meaning attached.
fn sorted(verb: &str, arguments: &Value) -> Sort {
    match verb {
        "ask" => Sort::Question,
        "reply" => Sort::Answer,
        "attention" => Sort::Attention,
        "trouble" => Sort::Trouble,
        "handoff" => Sort::Handoff,
        "claim" => Sort::Claim,
        "release" => Sort::Release,
        _ => arguments
            .get("sort")
            .and_then(Value::as_str)
            .and_then(Sort::read)
            .unwrap_or(Sort::Note),
    }
}

/// Everything decidable about a verb aimed at one instance.
///
/// `Err` is the refusal to hand back; `Ok` is a call worth making.
pub fn decide(
    verb: &str,
    who: &str,
    arguments: &Value,
    standing: &Standing,
) -> Result<Wanted, Output> {
    let Some(address) = Address::read(who) else {
        return Err(Output::error(format!(
            "`{who}` is not a name an instance can have. Names are `id`, `role/id` or \
             `project/role/id`."
        )));
    };
    if !VERBS.iter().any(|(name, _)| *name == verb) {
        return Err(Output::error(format!(
            "`{verb}` is not one of {TOOL}'s verbs. Call it with `verb: \"help\"`."
        )));
    }
    let reach = match verb {
        "stop" => Reach::Stop,
        _ if SPEAKS.contains(&verb) => Reach::Tell,
        _ => Reach::Ask,
    };
    let me = standing.whom();
    let whole = address.against(&standing.identity());
    let relation = standing.stands(&whole);
    if !policy::may(&me, relation, reach) {
        return Err(Output::error(policy::refusal(&me, relation, reach)));
    }
    let token = if reach == Reach::Stop {
        // Held only for what this session started. Refused here rather than at the far end,
        // where the answer would be "that is not the secret" — true, and no help at all to a
        // model that never had one.
        let Some(secret) = standing.minted.get(&whole.id).cloned() else {
            return Err(Output::error(format!(
                "this session did not start `{}`, so it holds nothing that could stop it",
                whole.full()
            )));
        };
        Some(secret)
    } else {
        None
    };
    Ok(Wanted {
        verb: verb.to_owned(),
        who: whole,
        sort: sorted(verb, arguments),
        message: arguments
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        about: arguments
            .get("about")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        token,
    })
}

/// Make the call, and say what came back.
pub fn perform(wanted: &Wanted, standing: &Standing) -> Output {
    let me = standing.identity();
    let mut held = match asking::Held::to(&wanted.who, &me) {
        Ok(held) => held,
        // The common failure, and worth its own sentence: a socket file outlives the process
        // that made it, so a name found in the directory is not a promise that anything is
        // behind it. "nothing is listening" is actionable; "connection refused" is not.
        Err(why) => {
            return Output::error(format!(
                "nothing is listening as `{}` ({why}). Use `list` to see who is actually there.",
                wanted.who.full()
            ));
        }
    };
    let reply = match said(&mut held, wanted) {
        Ok(reply) => reply,
        Err(why) => {
            return Output::error(format!("`{}` did not answer: {why}", wanted.who.full()));
        }
    };
    if !reply.ok {
        // The far end's own words. It knows things this side does not — that it was never
        // started by anybody, that the secret was wrong — and repeating them beats a summary.
        return Output::error(format!(
            "`{}` refused: {}",
            wanted.who.full(),
            reply.error.unwrap_or_else(|| "no reason given".to_owned())
        ));
    }
    Output::ok(landed(wanted, &reply))
}

/// One verb, over an open connection.
fn said(held: &mut asking::Held, wanted: &Wanted) -> std::io::Result<Reply> {
    match wanted.verb.as_str() {
        "about" => held.call("identity", Vec::new()),
        "status" => held.call("status", Vec::new()),
        "verbs" => held.call("verbs", Vec::new()),
        "stop" => held.call_with(
            "stop",
            Vec::new(),
            wanted.token.as_deref().unwrap_or_default(),
        ),
        // Everything else is a message. The sort is what makes them different verbs rather than
        // a wording choice: `attention` and `note` travel identically and mean entirely
        // different things to whoever reads them.
        _ => held.call(
            "tell",
            vec![
                serde_json::json!(wanted.message.clone().unwrap_or_default()),
                serde_json::json!(name_of(wanted.sort)),
                serde_json::json!(wanted.about),
            ],
        ),
    }
}

/// What to tell the model when it worked.
fn landed(wanted: &Wanted, reply: &Reply) -> String {
    let who = wanted.who.full();
    match wanted.verb.as_str() {
        // A report is whatever came back, verbatim. The model asked a question about another
        // session; summarising the answer here would be this file deciding what mattered.
        "about" | "status" | "verbs" => reply
            .result
            .first()
            .map_or_else(|| "nothing".to_owned(), ToString::to_string),
        "stop" => format!("`{who}` was told to stop."),
        // Said rather than assumed. `send` returning silently reads as though nothing happened,
        // and the one thing worth knowing is that it is in their inbox and not yet read.
        "ask" => format!(
            "The question is in `{who}`'s inbox. Its answer will arrive in yours; \
             it will not interrupt this turn."
        ),
        "attention" | "trouble" => {
            format!("`{who}` has it, marked so it can interrupt whatever they are doing.")
        }
        _ => format!("In `{who}`'s inbox."),
    }
}

/// The wire name of a sort.
fn name_of(sort: Sort) -> String {
    serde_json::to_value(sort)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "note".to_owned())
}

/// Deciding is settled before anything is dialled.
#[cfg(test)]
mod tests {
    use super::*;

    fn standing() -> Standing {
        Standing {
            me: "axon/main/alpha-rho".to_owned(),
            parent: None,
            forked: Vec::new(),
            minted: std::collections::BTreeMap::new(),
            inbox: Vec::new(),
        }
    }

    #[test]
    fn a_verb_is_decided_without_anything_listening() {
        // The whole reason deciding and doing are separate functions. Every refusal worth
        // testing is decidable, and a test that has to bind a socket is one nobody writes.
        let wanted = decide(
            "send",
            "beta-nu",
            &serde_json::json!({"message": "hello"}),
            &standing(),
        )
        .expect("a call worth making");
        assert_eq!(wanted.who.full(), "axon/main/beta-nu");
        assert_eq!(wanted.sort, Sort::Note);
    }

    #[test]
    fn the_verb_is_the_sort_for_everything_but_send() {
        for (verb, sort) in [
            ("ask", Sort::Question),
            ("reply", Sort::Answer),
            ("attention", Sort::Attention),
            ("trouble", Sort::Trouble),
            ("handoff", Sort::Handoff),
            ("claim", Sort::Claim),
            ("release", Sort::Release),
        ] {
            let wanted = decide(
                verb,
                "beta-nu",
                &serde_json::json!({"message": "x"}),
                &standing(),
            )
            .expect("decided");
            assert_eq!(wanted.sort, sort, "{verb}");
        }
    }

    #[test]
    fn send_takes_the_sort_it_was_given_and_defaults_to_a_note() {
        let given = decide(
            "send",
            "beta-nu",
            &serde_json::json!({"message": "x", "sort": "attention"}),
            &standing(),
        )
        .expect("decided");
        assert_eq!(given.sort, Sort::Attention);
        let bare = decide(
            "send",
            "beta-nu",
            &serde_json::json!({"message": "x"}),
            &standing(),
        )
        .expect("decided");
        assert_eq!(bare.sort, Sort::Note);
    }

    #[test]
    fn stopping_something_with_no_secret_is_refused_before_the_round_trip() {
        // The far end would say "that is not the secret", which is true and no help at all to a
        // model that never had one.
        let mut standing = standing();
        standing.forked.push("iota-mu".to_owned());
        let refused = decide("stop", "iota-mu", &serde_json::json!({}), &standing)
            .expect_err("nothing to stop it with");
        assert!(refused.is_error);
        assert!(
            refused.content.contains("did not start"),
            "{}",
            refused.content
        );
    }

    #[test]
    fn stopping_something_this_session_started_carries_the_secret() {
        let mut standing = standing();
        standing.forked.push("iota-mu".to_owned());
        standing
            .minted
            .insert("iota-mu".to_owned(), "s3cret".to_owned());
        let wanted = decide("stop", "iota-mu", &serde_json::json!({}), &standing).expect("decided");
        assert_eq!(wanted.token.as_deref(), Some("s3cret"));
    }

    #[test]
    fn no_other_verb_ever_carries_a_secret() {
        // One line that sends one, so it can be read. A verb picking one up by accident is how
        // a secret ends up somewhere it was never meant to go.
        let mut standing = standing();
        standing
            .minted
            .insert("beta-nu".to_owned(), "s3cret".to_owned());
        for verb in ["send", "ask", "status", "about", "attention"] {
            let wanted = decide(
                verb,
                "beta-nu",
                &serde_json::json!({"message": "x"}),
                &standing,
            )
            .expect("decided");
            assert!(wanted.token.is_none(), "{verb} carried the secret");
        }
    }

    #[test]
    fn a_wall_is_met_here_rather_than_over_the_socket() {
        let mut standing = standing();
        standing.parent = Some("beta-nu".to_owned());
        let refused = decide(
            "send",
            "other/main/tau-chi",
            &serde_json::json!({"message": "x"}),
            &standing,
        )
        .expect_err("across the wall");
        assert!(refused.content.contains("projects"), "{}", refused.content);
    }

    #[test]
    fn nothing_listening_says_so_and_says_what_to_do_about_it() {
        // A socket file outlives the process that made it, so a name in the directory is not a
        // promise that anything is behind it.
        let wanted = decide(
            "status",
            "nobody-nowhere",
            &serde_json::json!({}),
            &standing(),
        )
        .expect("decided");
        let out = perform(&wanted, &standing());
        assert!(out.is_error);
        assert!(
            out.content.contains("nothing is listening"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("list"),
            "and what to do: {}",
            out.content
        );
    }
}
