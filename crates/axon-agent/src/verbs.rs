//! The tool the model calls to reach other instances.
//!
//! This is the interface. Naming `$main/delta` in a prompt does not send anything — it tells the
//! model that instance exists and that this tool reaches it, and the model decides what to do.
//! Whether "tell $gamma to stop" means relay a sentence, ask a question first, or finish reading
//! a file before doing either is the model's judgement, and a harness that acted on the sentence
//! itself would be making that judgement badly and invisibly.
//!
//! # One tool, many verbs
//!
//! One entry in the tool list rather than eight, because a model given `agent_list`,
//! `agent_send`, `agent_status`… spends its attention choosing between names that differ by a
//! suffix. `verb` says which; `help` lists them all, and is the first thing the briefing points
//! at.
//!
//! # Stopping is the one that needs authority
//!
//! Everything here can be done to anything listening except `stop`. A session is stopped by the
//! session that started it and by nothing else — and "is" is not something a caller gets to
//! claim. A child is handed a secret in [`crate::directory::TOKEN`] at spawn, and a `stop` that
//! cannot quote it back is refused however convincing the name on it was.
//!
//! # Two walls, and what the model is shown
//!
//! `list` shows what this session can actually reach, never everything that exists. A model
//! told about a cousin it will then be refused spends the turn planning around a wall it was
//! never going to get through; see [`crate::policy`] for where the walls are.

pub mod doing;
pub mod saying;

use crate::directory::TOOL;
use crate::identity::Identity;
use crate::policy::{self, Relation, Whom};
use crate::wire::Message;
use serde_json::{Value, json};

/// What a verb produced, for a host to turn into whatever a tool result looks like there.
///
/// Not a tool trait, and that is the point of the file. This crate does not know what a tool is
/// — a harness does — and the moment it implemented one, it would depend on that harness and
/// could not leave. So the vocabulary is [`described`] as data, the work is [`answer`], and the
/// forty lines that make the two into a tool live on the other side of the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// What to tell the model.
    pub said: String,
    /// Whether it failed. A verb that ran and reported a problem is still an answer.
    pub failed: bool,
}

impl Answer {
    /// It worked.
    #[must_use]
    pub fn said(said: impl Into<String>) -> Self {
        Self {
            said: said.into(),
            failed: false,
        }
    }

    /// It did not, and this is why.
    #[must_use]
    pub fn refused(why: impl Into<String>) -> Self {
        Self {
            said: why.into(),
            failed: true,
        }
    }
}

/// The tool this crate offers, as a host needs to declare it.
///
/// Handed over as data rather than as an implementation, the way aeon publishes its descriptors
/// to axon: the vocabulary is written once, here, and a harness registers what comes back. Two
/// copies of nineteen verb descriptions would disagree the first time one was edited.
#[must_use]
pub fn described() -> Value {
    json!({
        "name": TOOL,
        "description": "Talk to other axon instances. `verb: \"help\"` lists everything this \
                        can do. Instances are named `id`, `role/id` or `project/role/id`; a \
                        bare id means one in this project. Use `list` to find out who is there \
                        and what may be done to each, rather than assuming a name.",
        "parameters": parameters(),
    })
}

/// The JSON Schema for a call.
#[must_use]
pub fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verb": {
                "type": "string",
                "enum": VERBS.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                "description": "what to do",
            },
            "who": {
                "type": "string",
                "description": "which instance, as `iota-mu`, `review/iota-mu` or \
                                `axon/review/iota-mu`. Not needed by `help`, `list` or \
                                `inbox`.",
            },
            "message": {
                "type": "string",
                "description": "what to say. Needed by send, ask, reply, announce, \
                                attention, trouble, handoff and claim.",
            },
            "about": {
                "type": "string",
                "description": "the id of the message being answered or released. \
                                Required by `reply`; `inbox` lists the ids.",
            },
            "sort": {
                "type": "string",
                "enum": ["note", "question", "answer", "attention", "claim", "release",
                         "handoff", "trouble"],
                "description": "for `send` only, what kind of message it is. The other \
                                verbs each mean one already. Defaults to note.",
            },
        },
        "required": ["verb"],
    })
}

/// What the tool can be asked to do.
///
/// Extensive on purpose. The narrow version — send, and stop — makes a model that wants to know
/// whether a sibling is even alive send it a message and wait to see what happens.
const VERBS: &[(&str, &str)] = &[
    // Knowing where you are. A subagent that does not know it is one cannot behave like one.
    ("help", "list these verbs and what each takes"),
    (
        "whoami",
        "this session's own name, who started it, and what it started",
    ),
    (
        "list",
        "every instance listening, and how each relates to this one",
    ),
    (
        "about",
        "who an instance is: project, role, id, and who started it",
    ),
    (
        "status",
        "whether an instance is working, for how long, and what is waiting for it",
    ),
    (
        "verbs",
        "what an instance says it can answer, asked of it rather than assumed",
    ),
    // Saying things. `send` returns at once; `ask` waits for the answer.
    ("send", "put a note in an instance's inbox and carry on"),
    ("ask", "ask an instance a question and wait for its answer"),
    (
        "reply",
        "answer a question that was asked of this session, quoting its id",
    ),
    ("announce", "send the same note to every instance listening"),
    // Asking for something. The difference between these and `send` is what the far end does
    // when it arrives, which is why they are verbs rather than a wording choice.
    (
        "attention",
        "tell an instance you need it — the one message allowed to interrupt a turn",
    ),
    (
        "trouble",
        "report that something is wrong and this session cannot go on",
    ),
    (
        "handoff",
        "give a piece of work to an instance: it is theirs now, not copied",
    ),
    // Not treading on each other. Advisory: axon records a claim, it does not enforce one.
    (
        "claim",
        "say this session is taking a piece of work, so others leave it alone",
    ),
    ("release", "say it is finished with, or was never started"),
    ("claims", "what every instance has said it is working on"),
    // Reading what came back.
    (
        "inbox",
        "what has been sent to this session and not yet acted on",
    ),
    (
        "history",
        "everything that has passed between this session and one instance",
    ),
    // Lifetime.
    (
        "stop",
        "end an instance this session started — refused for any it did not",
    ),
];

/// Which verbs need an instance named, and which do not.
///
/// A table rather than a condition per verb, because the third one written by hand disagreed
/// with the schema and the model was told `whoami` needed a `who`.
const ALONE: &[&str] = &[
    "help", "whoami", "list", "inbox", "claims", "announce", "trouble", "reply",
];

/// Which verbs need something said.
const SPEAKS: &[&str] = &[
    "send",
    "ask",
    "reply",
    "announce",
    "attention",
    "trouble",
    "handoff",
    "claim",
];

/// What the tool needs from the session in order to answer.
///
/// Handed in rather than reached for, because a tool runs on the turn thread and the session is
/// the UI's. What is here is a copy taken when the call started.
#[derive(Debug, Clone, Default)]
pub struct Standing {
    /// Who this session is.
    pub me: String,
    /// Who started it, if anybody.
    pub parent: Option<String>,
    /// The ids of what it started, which is what it may stop.
    ///
    /// Ids rather than whole names, because that is what the directory holds: a role is not on
    /// disk, so a full name built from it would carry a guess.
    pub forked: Vec<String>,
    /// The secret handed to each of them at spawn, by id, which a `stop` has to quote back.
    pub minted: std::collections::BTreeMap<String, String>,
    /// What has arrived.
    pub inbox: Vec<Message>,
}

impl Standing {
    /// This session as an identity, for filling the gaps in a short name.
    #[must_use]
    pub fn identity(&self) -> Identity {
        Identity::read(&self.me).unwrap_or_else(|| Identity {
            project: String::new(),
            role: "main".to_owned(),
            id: String::new(),
        })
    }

    /// Where this session sits in the tree.
    ///
    /// Project and id, and no role: what a session is *for* has no bearing on what it may reach.
    /// A session that could pick its own role could pick `main` and claim a main's reach, so
    /// the relation is worked out from the spawn tree and nothing else.
    #[must_use]
    pub fn whom(&self) -> Whom {
        let me = self.identity();
        Whom {
            project: me.project,
            id: me.id,
            parent: self.parent.clone(),
        }
    }

    /// How `them` stands to this session.
    ///
    /// What this session *started* comes first and is not up for discussion. The rest is read
    /// off the project directory, never from what the far end says about itself — a session
    /// that could describe its own place in the tree could describe itself as somebody's child.
    /// A child that declined to leave its note beside its socket would otherwise have made
    /// itself unstoppable by forgetting who its parent was.
    #[must_use]
    pub fn stands(&self, them: &Identity) -> Relation {
        let me = self.whom();
        if me.project != them.project {
            return Relation::Elsewhere;
        }
        // By id. What this session started is a list of ids, because that is what the directory
        // holds — a role is not on disk, and matching whole names would have missed a child
        // that called itself something this session did not expect.
        if self.forked.contains(&them.id) {
            return Relation::Child;
        }
        policy::between(&me, &crate::directory::whom(&them.project, &them.id))
    }
}

/// Answer one call.
///
/// `standing` is what the host knows about this session: its name, who started it, what it
/// started, and what has arrived. Handed in rather than reached for, so this stays a function
/// of its arguments and the host keeps the state.
#[must_use]
pub fn answer(arguments: &Value, standing: &Standing) -> Answer {
    let verb = arguments.get("verb").and_then(Value::as_str).unwrap_or("");
    if verb.is_empty() {
        return Answer::refused(format!(
            "{TOOL} needs a verb. Call it with `verb: \"help\"` to see them."
        ));
    }
    if !VERBS.iter().any(|(name, _)| *name == verb) {
        return Answer::refused(format!(
            "`{verb}` is not one of {TOOL}'s verbs. Call it with `verb: \"help\"`."
        ));
    }
    // What a verb needs is a table, not a condition per verb: the third one written by hand
    // disagreed with the schema and told the model `whoami` wanted a `who`.
    let said = arguments.get("message").and_then(Value::as_str);
    if SPEAKS.contains(&verb) && said.is_none_or(str::is_empty) {
        return Answer::refused(format!("`{verb}` needs `message` — what to say."));
    }
    if verb == "reply" && arguments.get("about").is_none() {
        return Answer::refused(
            "`reply` needs `about` — the id of the message being answered. `inbox` lists them."
                .to_owned(),
        );
    }
    match verb {
        "help" => Answer::said(saying::help(standing)),
        "whoami" => Answer::said(saying::whoami(standing)),
        "list" => Answer::said(saying::list(standing)),
        "inbox" => Answer::said(saying::inbox(standing)),
        _ if ALONE.contains(&verb) => Answer::said(format!(
            "`{verb}` is understood but not yet carried out: the socket call it makes is \
                 not wired into the turn loop."
        )),
        _ => match arguments.get("who").and_then(Value::as_str) {
            // Decided first, dialled second. Everything worth refusing is refused before
            // the round trip, so a model that asked for something it may not have is told
            // what it may do instead of paying for the answer.
            Some(who) => match doing::decide(verb, who, arguments, standing) {
                Ok(wanted) => doing::perform(&wanted, standing),
                Err(refused) => refused,
            },
            None => Answer::refused(format!("`{verb}` needs `who` — which instance to reach.")),
        },
    }
}

/// The tool refuses what it should and asks for what it needs.
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

    fn call(arguments: Value, standing: Standing) -> Answer {
        answer(&arguments, &standing)
    }

    #[test]
    fn help_lists_every_verb() {
        // The first thing the briefing points the model at, so it has to be complete.
        let out = call(json!({"verb": "help"}), standing());
        assert!(!out.failed);
        assert!(
            out.said.contains("axon/main/alpha-rho"),
            "it never says who we are"
        );
        for (verb, _) in VERBS {
            assert!(out.said.contains(verb), "{verb} is missing from help");
        }
    }

    #[test]
    fn the_schema_offers_exactly_the_verbs_that_exist() {
        // A model told about a verb the tool does not have spends a call finding out.
        let schema = parameters();
        let offered = schema["properties"]["verb"]["enum"]
            .as_array()
            .expect("an enum")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let held: Vec<&str> = VERBS.iter().map(|(name, _)| *name).collect();
        assert_eq!(offered, held);
    }

    #[test]
    fn a_verb_that_needs_an_instance_says_so_when_it_has_none() {
        let out = call(json!({"verb": "status"}), standing());
        assert!(out.failed);
        assert!(out.said.contains("who"), "{}", out.said);
    }

    #[test]
    fn help_and_inbox_need_nobody() {
        for verb in ["help", "inbox"] {
            let out = call(json!({"verb": verb}), standing());
            assert!(!out.failed, "{verb}: {}", out.said);
        }
    }

    #[test]
    fn stopping_something_this_session_did_not_start_is_refused_here() {
        // Refused before the round trip, so the model is told what it may do instead of
        // spending a turn discovering it.
        let out = call(json!({"verb": "stop", "who": "beta-nu"}), standing());
        assert!(out.failed);
        assert!(out.said.contains("started"), "{}", out.said);
    }

    #[test]
    fn stopping_something_this_session_started_gets_past_both_gates() {
        // Two gates, and this proves it clears both: the relation says it is this session's
        // child, and the secret says this session is the one that started it. What stops it
        // here is the socket, because nothing is listening in a test — and that failure
        // arriving *is* the evidence, since a refusal would have come before the dial.
        let mut standing = standing();
        standing.forked.push("iota-mu".to_owned());
        standing
            .minted
            .insert("iota-mu".to_owned(), "s3cret".to_owned());
        let out = call(json!({"verb": "stop", "who": "iota-mu"}), standing);
        assert!(out.failed);
        assert!(
            out.said.contains("nothing is listening"),
            "it was refused before it got to the socket: {}",
            out.said
        );
    }

    #[test]
    fn a_child_is_what_this_session_started_and_not_what_a_name_looks_like() {
        // The authority comes from what this session remembers doing, never from the far end's
        // description of itself. A child that declined to leave its note beside its socket
        // would otherwise have made itself unstoppable by forgetting who its parent was.
        let mut standing = standing();
        standing.forked.push("iota-mu".to_owned());
        let child = Identity {
            project: "axon".to_owned(),
            role: "main".to_owned(),
            id: "iota-mu".to_owned(),
        };
        let stranger = Identity {
            project: "axon".to_owned(),
            role: "main".to_owned(),
            id: "beta-nu".to_owned(),
        };
        assert_eq!(standing.stands(&child), Relation::Child);
        assert_ne!(standing.stands(&stranger), Relation::Child);
    }

    #[test]
    fn nothing_in_another_project_is_a_child_however_it_was_recorded() {
        // The project wall wins over local memory, so a stale entry cannot reach across it.
        let mut standing = standing();
        standing.forked.push("other/iota-mu".to_owned());
        let across = Identity {
            project: "other".to_owned(),
            role: "main".to_owned(),
            id: "iota-mu".to_owned(),
        };
        assert_eq!(standing.stands(&across), Relation::Elsewhere);
    }

    #[test]
    fn sending_without_anything_to_say_is_refused() {
        for verb in ["send", "ask"] {
            let out = call(json!({"verb": verb, "who": "gamma"}), standing());
            assert!(out.failed, "{verb} sent nothing");
            assert!(out.said.contains("message"), "{}", out.said);
        }
    }

    #[test]
    fn an_unknown_verb_points_at_help() {
        let out = call(json!({"verb": "obliterate", "who": "gamma"}), standing());
        assert!(out.failed);
        assert!(out.said.contains("help"), "{}", out.said);
    }

    #[test]
    fn a_name_nothing_can_have_says_what_a_name_looks_like() {
        let out = call(json!({"verb": "status", "who": "a/b/c/d"}), standing());
        assert!(out.failed);
        assert!(out.said.contains("project/role/id"), "{}", out.said);
    }

    #[test]
    fn the_inbox_names_who_sent_what() {
        let mut standing = standing();
        standing
            .inbox
            .push(Message::new("axon/main/gamma", "the parser is fixed"));
        let out = call(json!({"verb": "inbox"}), standing);
        assert!(out.said.contains("axon/main/gamma"), "{}", out.said);
        assert!(out.said.contains("the parser is fixed"));
    }
}

/// This session's own name and place.
/// The wider surface: what each verb needs, and what it means when it lands.
#[cfg(test)]
mod surface_tests {
    use super::*;
    use crate::wire::Sort;

    fn standing() -> Standing {
        Standing {
            me: "axon/main/alpha-rho".to_owned(),
            parent: None,
            forked: Vec::new(),
            minted: std::collections::BTreeMap::new(),
            inbox: Vec::new(),
        }
    }

    fn call(arguments: Value, standing: Standing) -> Answer {
        answer(&arguments, &standing)
    }

    #[test]
    fn every_verb_either_needs_an_instance_or_is_listed_as_not_needing_one() {
        // The table and the dispatch drift the moment either is edited, and the drift shows up
        // as the model being told `whoami` wants a `who`.
        for (verb, _) in VERBS {
            let out = call(
                json!({"verb": verb, "message": "x", "about": "y"}),
                standing(),
            );
            let wants_who = out.failed && out.said.contains("needs `who`");
            assert_eq!(
                wants_who,
                !ALONE.contains(verb),
                "{verb}: needs an instance = {wants_who}, listed as alone = {}",
                ALONE.contains(verb)
            );
        }
    }

    #[test]
    fn a_verb_that_says_something_refuses_to_say_nothing() {
        for verb in SPEAKS {
            let out = call(json!({"verb": verb, "who": "gamma"}), standing());
            assert!(out.failed, "{verb} sent an empty message");
            assert!(out.said.contains("message"), "{verb}: {}", out.said);
        }
    }

    #[test]
    fn a_reply_has_to_say_what_it_is_answering() {
        // Without it the far end has an answer and no idea to what, which is worse than no
        // answer: it reads as an unprompted assertion.
        let out = call(json!({"verb": "reply", "message": "yes"}), standing());
        assert!(out.failed);
        assert!(out.said.contains("about"), "{}", out.said);
        assert!(out.said.contains("inbox"), "and where to find the id");
    }

    #[test]
    fn a_root_session_is_told_it_has_nobody_to_escalate_to() {
        // A subagent that does not know it is one will not raise `attention` at a parent it
        // does not know it has. A root that thinks it has one will wait for an answer forever.
        let said = call(json!({"verb": "whoami"}), standing()).said;
        assert!(said.contains("root session"), "{said}");

        let mut child = standing();
        child.parent = Some("axon/main/root".to_owned());
        let said = call(json!({"verb": "whoami"}), child).said;
        assert!(said.contains("axon/main/root"), "{said}");
        assert!(said.contains("attention"), "and what to do with it: {said}");
    }

    #[test]
    fn whoami_says_what_it_may_stop() {
        let mut standing = standing();
        standing.forked.push("axon/main/gamma".to_owned());
        let said = call(json!({"verb": "whoami"}), standing).said;
        assert!(said.contains("axon/main/gamma"), "{said}");
        assert!(said.contains("may stop"), "{said}");
    }

    #[test]
    fn only_a_cry_for_help_interrupts() {
        // An inbox that interrupts for every note is an inbox nobody leaves switched on.
        for sort in [Sort::Attention, Sort::Trouble] {
            assert!(sort.interrupts(), "{sort:?} should reach a busy session");
        }
        for sort in [Sort::Note, Sort::Question, Sort::Answer, Sort::Claim] {
            assert!(!sort.interrupts(), "{sort:?} should wait");
        }
    }

    #[test]
    fn the_inbox_marks_what_is_urgent_and_what_is_owed_an_answer() {
        let mut standing = standing();
        standing.inbox.push(Message::new("axon/main/beta", "fyi"));
        standing.inbox.push(Message::sent(
            "axon/main/gamma",
            "which parser?",
            Sort::Question,
            None,
        ));
        standing.inbox.push(Message::sent(
            "axon/main/delta",
            "I am stuck",
            Sort::Attention,
            None,
        ));
        let said = call(json!({"verb": "inbox"}), standing).said;
        assert!(
            said.contains("! `axon/main/delta`"),
            "urgent unmarked: {said}"
        );
        assert!(
            said.contains("`reply`"),
            "no way back to the question: {said}"
        );
        assert!(said.contains("[question]"), "sorts are not shown: {said}");
    }

    #[test]
    fn a_message_can_be_answered_by_the_id_the_inbox_showed() {
        // The id has to survive the round trip, or `reply` quotes something nobody has.
        let message = Message::sent("axon/main/gamma", "which parser?", Sort::Question, None);
        let text = serde_json::to_string(&message).expect("encodes");
        let back: Message = serde_json::from_str(&text).expect("decodes");
        assert_eq!(back.id, message.id);
        assert_eq!(back.sort, Sort::Question);
    }
}
