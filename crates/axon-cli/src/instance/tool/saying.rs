//! What the tool says back.
//!
//! The verbs that only report — no socket, no far end, nothing but this session reading
//! itself out. Split from [`super`] because the two answer different questions: that file is
//! about who may do what, and this one is about how it reads when they may.

use super::{Standing, TOOL, VERBS};
use crate::instance::policy::Relation;

/// Every verb, as the model should read it.
pub fn help(standing: &Standing) -> String {
    let rows: Vec<String> = VERBS
        .iter()
        .map(|(name, does)| format!("- `{name}` — {does}"))
        .collect();
    format!(
        "This session is `{me}`{born}.\n\n`{TOOL}` verbs:\n\n{rows}\n\nInstances are named \
         `project/id`. A bare id means one in this project, and there is no reaching outside \
         it.\n\nWho can be reached at all depends on where each of you sits: `list` says, and \
         it lists only what this session may actually reach. Only an instance this session \
         started can be stopped.",
        me = standing.me,
        born = standing
            .parent
            .as_ref()
            .map_or(String::new(), |who| format!(", started by `{who}`")),
        rows = rows.join("\n")
    )
}

/// Who is there, and how each of them stands to this session.
///
/// Only what can actually be reached. A model told about a cousin it will then be refused
/// spends the turn planning around a wall it was never going to get through, and the refusal
/// arrives too late to change the plan.
pub fn list(standing: &Standing) -> String {
    let me = standing.whom();
    let there = crate::instance::reachable(&me);
    if there.is_empty() {
        return format!(
            "Nothing else in `{}` can be reached from here. Either nothing else is running, \
             or what is running is behind another instance's door — see `whoami`.",
            me.project
        );
    }
    let rows: Vec<String> = there
        .into_iter()
        .map(|(them, relation)| {
            let stoppable = if relation == Relation::Child {
                ", which this session may stop"
            } else {
                ""
            };
            format!(
                "- `{}/{}` — {}{stoppable}",
                them.project,
                them.id,
                relation.named()
            )
        })
        .collect();
    format!(
        "In `{}`, reachable from here:\n\n{}",
        me.project,
        rows.join("\n")
    )
}

/// What has been sent here.
pub fn inbox(standing: &Standing) -> String {
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

/// This session's own name and place.
///
/// A subagent that does not know it is one cannot behave like one: it will not think to raise
/// `attention` at a parent it does not know it has, and it will try to `stop` siblings it has
/// no authority over. This is the first thing such a session should ask.
pub fn whoami(standing: &Standing) -> String {
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
