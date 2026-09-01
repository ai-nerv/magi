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

use super::wire::Message;
use super::{Address, Kind, Reach, TOOL};
use axon_tools::{Cancel, Ops, Output, Tool};
use serde_json::{Value, json};

/// What the tool can be asked to do.
///
/// Extensive on purpose. The narrow version — send, and stop — makes a model that wants to know
/// whether a sibling is even alive send it a message and wait to see what happens.
const VERBS: &[(&str, &str)] = &[
    ("help", "list these verbs and what each one takes"),
    (
        "list",
        "every instance currently listening, and how it relates to this one",
    ),
    ("about", "who an instance is: its project, role and id"),
    (
        "status",
        "whether an instance is working, for how long, and how much is waiting for it",
    ),
    (
        "verbs",
        "what an instance itself says it can answer, asked of it rather than assumed",
    ),
    (
        "send",
        "put a message in an instance's inbox and return at once",
    ),
    (
        "ask",
        "send a message and wait for the instance to answer it",
    ),
    (
        "inbox",
        "what has been sent to this session and not yet acted on",
    ),
    (
        "history",
        "what has passed between this session and one instance",
    ),
    (
        "stop",
        "end an instance this session started — ignored by anything it did not",
    ),
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
                    "description": "what to say, for `send` and `ask`.",
                },
            },
            "required": ["verb"],
        })
    }

    fn run(&self, arguments: &Value, _ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let verb = arguments.get("verb").and_then(Value::as_str).unwrap_or("");
        match verb {
            "help" => Output::ok(help(&self.standing)),
            "inbox" => Output::ok(inbox(&self.standing)),
            // Everything else names an instance, and saying which is missing beats a schema
            // error the model has to guess its way out of.
            "" => Output::error(format!(
                "{TOOL} needs a verb. Call it with `verb: \"help\"` to see them."
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
    let rows: Vec<String> = standing
        .inbox
        .iter()
        .map(|message| format!("- from `{}`: {}", message.from, message.text))
        .collect();
    rows.join("\n")
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
    Output::ok(format!(
        "queued: {verb} {}",
        Wanted {
            verb: verb.to_owned(),
            who: address.written(),
            message: arguments
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        }
        .who
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
    /// What to say, for the verbs that say something.
    pub message: Option<String>,
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
