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
//! session that started it and by nothing else: a child learns its parent's name from
//! [`super::PARENT`] at spawn, and a `stop` from any other name is **ignored** rather than
//! refused loudly — a stranger who can tell the difference between "refused" and "no such
//! instance" has learned something about a session that is not theirs.

use super::wire::{Message, Sort};
use super::{Address, Kind, Reach, TOOL};
use axon_tools::{Cancel, Ops, Output, Tool};
use serde_json::{Value, json};

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
        "who an instance is: project, role, id, and its parent",
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
    /// What it started, which is what it may stop.
    pub forked: Vec<String>,
    /// What has arrived.
    pub inbox: Vec<Message>,
}

impl Standing {
    /// How this session may treat `whole`.
    ///
    /// A fork is one *this session started*. Nothing else is, whatever it says about itself —
    /// which is the point: the answer comes from what this session remembers doing, never from
    /// the far end's description of itself.
    #[must_use]
    pub fn kind(&self, whole: &str) -> Kind {
        if self.forked.iter().any(|child| child == whole) {
            Kind::Fork
        } else {
            Kind::Peer
        }
    }
}

/// Reaching other instances.
pub struct Agent {
    /// What this session knows about itself and its neighbours.
    pub standing: Standing,
}

impl Tool for Agent {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        "Talk to other axon instances. `verb: \"help\"` lists everything this can do. \
         Instances are named `project/role/id`, and a short name fills the rest in from this \
         session — `gamma` is a sibling, `main/delta` names the role. Use `list` to find out \
         who is there before assuming a name."
    }

    fn parameters(&self) -> Value {
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
                    "description": "which instance, as `gamma`, `main/delta` or \
                                    `project/role/id`. Not needed by `help`, `list` or `inbox`.",
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

    fn run(&self, arguments: &Value, _ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let verb = arguments.get("verb").and_then(Value::as_str).unwrap_or("");
        if verb.is_empty() {
            return Output::error(format!(
                "{TOOL} needs a verb. Call it with `verb: \"help\"` to see them."
            ));
        }
        if !VERBS.iter().any(|(name, _)| *name == verb) {
            return Output::error(format!(
                "`{verb}` is not one of {TOOL}'s verbs. Call it with `verb: \"help\"`."
            ));
        }
        // What a verb needs is a table, not a condition per verb: the third one written by hand
        // disagreed with the schema and told the model `whoami` wanted a `who`.
        let said = arguments.get("message").and_then(Value::as_str);
        if SPEAKS.contains(&verb) && said.is_none_or(str::is_empty) {
            return Output::error(format!("`{verb}` needs `message` — what to say."));
        }
        if verb == "reply" && arguments.get("about").is_none() {
            return Output::error(
                "`reply` needs `about` — the id of the message being answered. `inbox` lists them."
                    .to_owned(),
            );
        }
        match verb {
            "help" => Output::ok(help(&self.standing)),
            "whoami" => Output::ok(whoami(&self.standing)),
            "inbox" => Output::ok(inbox(&self.standing)),
            _ if ALONE.contains(&verb) => Output::ok(format!(
                "`{verb}` is understood but not yet carried out: the socket call it makes is \
                 not wired into the turn loop."
            )),
            _ => match arguments.get("who").and_then(Value::as_str) {
                Some(who) => reach(verb, who, arguments, &self.standing),
                None => Output::error(format!("`{verb}` needs `who` — which instance to reach.")),
            },
        }
    }
}

/// Every verb, as the model should read it.
fn help(standing: &Standing) -> String {
    let rows: Vec<String> = VERBS
        .iter()
        .map(|(name, does)| format!("- `{name}` — {does}"))
        .collect();
    format!(
        "This session is `{me}`{born}.\n\n`{TOOL}` verbs:\n\n{rows}\n\nInstances are named \
         `project/role/id`. A short name fills the rest in from this session: `gamma` is a \
         sibling in the same project and role, `main/delta` names the role.\n\nAnything \
         listening can be asked and sent to. Only an instance this session started can be \
         stopped.",
        me = standing.me,
        born = standing
            .parent
            .as_ref()
            .map_or(String::new(), |who| format!(", started by `{who}`")),
        rows = rows.join("\n")
    )
}

/// What has been sent here.
fn inbox(standing: &Standing) -> String {
    if standing.inbox.is_empty() {
        return "Nothing has been sent to this session.".to_owned();
    }
    // Marked rather than sorted. Order is when things arrived, which is what makes a
    // conversation readable; the mark is what makes the urgent one findable in it.
    let rows: Vec<String> = standing
        .inbox
        .iter()
        .map(|message| {
            let mark = if message.sort.interrupts() { "! " } else { "" };
            let owed = if message.sort.expects_an_answer() {
                format!(" — answer with `reply`, `about: \"{}\"`", message.id)
            } else {
                String::new()
            };
            format!(
                "- {mark}`{}` [{}] {}{owed}",
                message.from,
                serde_json::to_value(message.sort)
                    .ok()
                    .and_then(|v| v.as_str().map(ToOwned::to_owned))
                    .unwrap_or_else(|| "note".to_owned()),
                message.text
            )
        })
        .collect();
    rows.join("\n")
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

/// Everything that needs an instance to act on.
///
/// The call itself is not made here: this crate's tools are synchronous and the socket is not,
/// so what comes back says what *would* be asked and of whom. The turn loop performs it — see
/// [`Wanted`], which is what it is handed.
fn reach(verb: &str, who: &str, arguments: &Value, standing: &Standing) -> Output {
    let Some(address) = Address::read(who) else {
        return Output::error(format!(
            "`{who}` is not a name an instance can have. Names are `id`, `role/id` or \
             `project/role/id`."
        ));
    };
    if !VERBS.iter().any(|(name, _)| *name == verb) {
        return Output::error(format!(
            "`{verb}` is not one of {TOOL}'s verbs. Call it with `verb: \"help\"`."
        ));
    }
    // Refused here rather than at the far end, so the model is told what it may do instead of
    // spending a turn discovering it. The far end refuses it too, because a caller is not to be
    // trusted with its own permissions.
    let wanted = match verb {
        "stop" => Reach::Stop,
        "send" | "ask" => Reach::Tell,
        _ => Reach::Ask,
    };
    if !wanted.allows(standing.kind(who)) {
        return Output::error(format!(
            "{}. It can still be asked and sent to.",
            wanted.refusal(&address)
        ));
    }
    if matches!(verb, "send" | "ask") && arguments.get("message").is_none() {
        return Output::error(format!("`{verb}` needs `message` — what to say."));
    }
    let wanted = Wanted {
        verb: verb.to_owned(),
        who: address.written(),
        sort: sorted(verb, arguments),
        message: arguments
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        about: arguments
            .get("about")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    };
    Output::ok(format!(
        "`{}` is understood but not yet carried out: the socket call it makes is not wired into \
         the turn loop.",
        wanted.verb
    ))
}

/// A call the tool decided on, for the turn loop to actually make.
///
/// The socket is asynchronous and [`Tool::run`] is not. Rather than block a turn thread on a
/// peer that may be mid-turn itself, the tool decides *what* to ask and hands it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// Which verb.
    pub verb: String,
    /// Which instance, written out.
    pub who: String,
    /// What kind of message it carries.
    pub sort: Sort,
    /// What to say, for the verbs that say something.
    pub message: Option<String>,
    /// The message being answered or released, for the verbs that quote one.
    pub about: Option<String>,
}

/// The tool refuses what it should and asks for what it needs.
#[cfg(test)]
mod tests {
    use super::*;

    fn standing() -> Standing {
        Standing {
            me: "axon/main/alpha".to_owned(),
            parent: None,
            forked: Vec::new(),
            inbox: Vec::new(),
        }
    }

    fn call(arguments: Value, standing: Standing) -> Output {
        Agent { standing }.run(
            &arguments,
            &axon_tools::ops::Real::new(std::path::PathBuf::from(".")),
            &axon_tools::Uncancelled,
        )
    }

    #[test]
    fn help_lists_every_verb() {
        // The first thing the briefing points the model at, so it has to be complete.
        let out = call(json!({"verb": "help"}), standing());
        assert!(!out.is_error);
        assert!(
            out.content.contains("axon/main/alpha"),
            "it never says who we are"
        );
        for (verb, _) in VERBS {
            assert!(out.content.contains(verb), "{verb} is missing from help");
        }
    }

    #[test]
    fn the_schema_offers_exactly_the_verbs_that_exist() {
        // A model told about a verb the tool does not have spends a call finding out.
        let schema = Agent {
            standing: standing(),
        }
        .parameters();
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
        assert!(out.is_error);
        assert!(out.content.contains("who"), "{}", out.content);
    }

    #[test]
    fn help_and_inbox_need_nobody() {
        for verb in ["help", "inbox"] {
            let out = call(json!({"verb": verb}), standing());
            assert!(!out.is_error, "{verb}: {}", out.content);
        }
    }

    #[test]
    fn stopping_something_this_session_did_not_start_is_refused_here() {
        // Refused before the round trip, so the model is told what it may do instead of
        // spending a turn discovering it.
        let out = call(json!({"verb": "stop", "who": "main/delta"}), standing());
        assert!(out.is_error);
        assert!(out.content.contains("peer"), "{}", out.content);
        assert!(out.content.contains("ask it"), "and what it can do instead");
    }

    #[test]
    fn stopping_a_fork_is_allowed() {
        let mut standing = standing();
        standing.forked.push("gamma".to_owned());
        let out = call(json!({"verb": "stop", "who": "gamma"}), standing);
        assert!(!out.is_error, "{}", out.content);
    }

    #[test]
    fn a_fork_is_what_this_session_started_and_not_what_a_name_looks_like() {
        // The authority comes from what this session remembers doing, never from the far end's
        // description of itself.
        let mut standing = standing();
        standing.forked.push("axon/main/gamma".to_owned());
        assert_eq!(standing.kind("axon/main/gamma"), Kind::Fork);
        assert_eq!(standing.kind("axon/main/delta"), Kind::Peer);
    }

    #[test]
    fn sending_without_anything_to_say_is_refused() {
        for verb in ["send", "ask"] {
            let out = call(json!({"verb": verb, "who": "gamma"}), standing());
            assert!(out.is_error, "{verb} sent nothing");
            assert!(out.content.contains("message"), "{}", out.content);
        }
    }

    #[test]
    fn an_unknown_verb_points_at_help() {
        let out = call(json!({"verb": "obliterate", "who": "gamma"}), standing());
        assert!(out.is_error);
        assert!(out.content.contains("help"), "{}", out.content);
    }

    #[test]
    fn a_name_nothing_can_have_says_what_a_name_looks_like() {
        let out = call(json!({"verb": "status", "who": "a/b/c/d"}), standing());
        assert!(out.is_error);
        assert!(out.content.contains("project/role/id"), "{}", out.content);
    }

    #[test]
    fn the_inbox_names_who_sent_what() {
        let mut standing = standing();
        standing
            .inbox
            .push(Message::new("axon/main/gamma", "the parser is fixed"));
        let out = call(json!({"verb": "inbox"}), standing);
        assert!(out.content.contains("axon/main/gamma"), "{}", out.content);
        assert!(out.content.contains("the parser is fixed"));
    }
}

/// This session's own name and place.
///
/// A subagent that does not know it is one cannot behave like one: it will not think to raise
/// `attention` at a parent it does not know it has, and it will try to `stop` siblings it has
/// no authority over. This is the first thing such a session should ask.
fn whoami(standing: &Standing) -> String {
    let mut said = format!("This session is `{}`.", standing.me);
    match &standing.parent {
        Some(who) => said.push_str(&format!(
            " It was started by `{who}`, which is the only session that can stop it — and the \
             one to raise `attention` at when this session needs a decision it cannot make."
        )),
        None => said.push_str(
            " Nothing started it, so it is a root session: no parent to escalate to, and \
             nothing can stop it from outside.",
        ),
    }
    if standing.forked.is_empty() {
        said.push_str(" It has started nothing, so there is nothing it may stop.");
    } else {
        said.push_str(&format!(
            " It started {}, which it may stop: {}.",
            standing.forked.len(),
            standing
                .forked
                .iter()
                .map(|child| format!("`{child}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    said
}

/// The wider surface: what each verb needs, and what it means when it lands.
#[cfg(test)]
mod surface_tests {
    use super::*;

    fn standing() -> Standing {
        Standing {
            me: "axon/main/alpha".to_owned(),
            parent: None,
            forked: Vec::new(),
            inbox: Vec::new(),
        }
    }

    fn call(arguments: Value, standing: Standing) -> Output {
        Agent { standing }.run(
            &arguments,
            &axon_tools::ops::Real::new(std::path::PathBuf::from(".")),
            &axon_tools::Uncancelled,
        )
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
            let wants_who = out.is_error && out.content.contains("needs `who`");
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
            assert!(out.is_error, "{verb} sent an empty message");
            assert!(out.content.contains("message"), "{verb}: {}", out.content);
        }
    }

    #[test]
    fn a_reply_has_to_say_what_it_is_answering() {
        // Without it the far end has an answer and no idea to what, which is worse than no
        // answer: it reads as an unprompted assertion.
        let out = call(json!({"verb": "reply", "message": "yes"}), standing());
        assert!(out.is_error);
        assert!(out.content.contains("about"), "{}", out.content);
        assert!(out.content.contains("inbox"), "and where to find the id");
    }

    #[test]
    fn a_root_session_is_told_it_has_nobody_to_escalate_to() {
        // A subagent that does not know it is one will not raise `attention` at a parent it
        // does not know it has. A root that thinks it has one will wait for an answer forever.
        let said = call(json!({"verb": "whoami"}), standing()).content;
        assert!(said.contains("root session"), "{said}");

        let mut child = standing();
        child.parent = Some("axon/main/root".to_owned());
        let said = call(json!({"verb": "whoami"}), child).content;
        assert!(said.contains("axon/main/root"), "{said}");
        assert!(said.contains("attention"), "and what to do with it: {said}");
    }

    #[test]
    fn whoami_says_what_it_may_stop() {
        let mut standing = standing();
        standing.forked.push("axon/main/gamma".to_owned());
        let said = call(json!({"verb": "whoami"}), standing).content;
        assert!(said.contains("axon/main/gamma"), "{said}");
        assert!(said.contains("may stop"), "{said}");
    }

    #[test]
    fn the_verb_is_the_sort_for_everything_but_send() {
        // `attention` and `send` travel identically and mean different things to whoever reads
        // them. If the verb did not set the sort, every one of them would arrive as a note.
        for (verb, want) in [
            ("ask", Sort::Question),
            ("reply", Sort::Answer),
            ("attention", Sort::Attention),
            ("trouble", Sort::Trouble),
            ("handoff", Sort::Handoff),
            ("claim", Sort::Claim),
            ("release", Sort::Release),
        ] {
            assert_eq!(sorted(verb, &json!({})), want, "{verb}");
        }
        assert_eq!(sorted("send", &json!({})), Sort::Note, "the general case");
        assert_eq!(
            sorted("send", &json!({"sort": "attention"})),
            Sort::Attention,
            "which can be told what it is"
        );
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
        let said = call(json!({"verb": "inbox"}), standing).content;
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
