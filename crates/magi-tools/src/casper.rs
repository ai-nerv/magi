//! Tools that live in casper.
//!
//! magi keeps `read`, `write` and `edit` — the floor it can never be without. Everything else is
//! casper's, and this is how it is reached: ask what exists, hand over a call, read back what it
//! produced.
//!
//! # Why a spawn and not a socket
//!
//! casper runs programs, and *a socket that runs commands is remote code execution*. So its
//! socket answers only what exists, and a call goes over the spawn link — argv and stdin, from a
//! process that could have run the command itself. One exec per call, which is the same shape
//! [`crate::process`] pays for and for the same reason: the boundary is the point.
//!
//! # What comes back is two things
//!
//! A tool result is read by the model *and* drawn for the person, and those are not the same
//! content. [`magi_proto::tooling::Ran`] carries both: `said` is what the model reads, and
//! `shown` is a painted view or a question. This module keeps `said`, because a [`Tool`] returns
//! text; the view is carried alongside by [`Ran::shown`] for the caller that draws.

use crate::question::Asks;
use crate::{Cancel, Ops, Output, Tool};
use magi_proto::tooling::{Call, Card, Ran, Shown};
use std::sync::Arc;

/// The program that owns the tools.
///
/// Found on `PATH`, like every other sibling. Named here rather than inline so a test can put
/// something else in its place without touching the environment: `PATH` is process-wide, and
/// tests that fought over it would be tests that pass alone and fail together.
pub const CASPER: &str = "casper";

/// What casper says it offers.
///
/// Empty when casper is not installed or would not answer. Not an error: a session without it
/// keeps the tools magi declares itself, exactly as it did before casper existed.
#[must_use]
pub fn cards() -> Vec<Card> {
    cards_from(CASPER)
}

/// The same, against a named program.
#[must_use]
pub fn cards_from(program: &str) -> Vec<Card> {
    let Ok(out) = std::process::Command::new(program)
        .arg("tools")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        magi_model::noted!("casper: {program} tools could not be started");
        return Vec::new();
    };
    rows(&out.stdout)
        .and_then(|rows| rows.first().cloned())
        .and_then(|first| serde_json::from_value(first).ok())
        .unwrap_or_default()
}

/// The rows of a family reply, or nothing when it was not one.
fn rows(body: &[u8]) -> Option<Vec<serde_json::Value>> {
    let reply: serde_json::Value = serde_json::from_slice(body).ok()?;
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    reply
        .get("result")
        .and_then(serde_json::Value::as_array)
        .cloned()
}

/// Hand one call to casper and read back what it produced.
///
/// # Errors
/// A refusal — casper could not be started, or would not take the call. Distinct from a tool that
/// *ran* and reported a problem, which comes back as [`Ran::failed`] and is something the model
/// should read.
pub fn run(program: &str, call: &Call) -> Result<Ran, String> {
    use std::io::Write;
    let body =
        serde_json::to_vec(call).map_err(|why| format!("this call will not encode: {why}"))?;

    let mut child = std::process::Command::new(program)
        .arg("run")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|why| {
            magi_model::noted!("casper: {program} run could not be started: {why}");
            format!("{program} could not be started: {why}")
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Written and closed. casper reads to end of file, so a handle left open is a call that
        // never starts.
        let _ = stdin.write_all(&body);
    }
    let out = child
        .wait_with_output()
        .map_err(|why| format!("{program} did not finish: {why}"))?;

    let rows = rows(&out.stdout).ok_or_else(|| {
        // The reply shape is what a client parses; anything else is a casper that answered
        // something this build cannot read, which is worth saying rather than swallowing.
        format!("{program} answered something unreadable")
    })?;
    let first = rows
        .first()
        .cloned()
        .ok_or_else(|| format!("{program} answered nothing"))?;
    serde_json::from_value(first).map_err(|why| format!("{program}: {why}"))
}

/// One of casper's tools, as magi's registry sees it.
pub struct CasperTool {
    card: Card,
    program: String,
    asks: Arc<dyn Asks>,
    holds: Arc<dyn crate::holding::Holds>,
}

impl CasperTool {
    /// Every tool casper offers, ready to register.
    ///
    /// `asks` is how a question reaches the person. A tool that never asks never uses it; one
    /// that does cannot finish without it, which is why it is taken here rather than looked up
    /// when the question arrives.
    #[must_use]
    pub fn all(
        program: &str,
        asks: Arc<dyn Asks>,
        holds: Arc<dyn crate::holding::Holds>,
    ) -> Vec<Self> {
        cards_from(program)
            .into_iter()
            .map(|card| Self {
                card,
                program: program.to_owned(),
                asks: Arc::clone(&asks),
                holds: Arc::clone(&holds),
            })
            .collect()
    }
}

impl Tool for CasperTool {
    fn composition(&self) -> Vec<(&'static str, String)> {
        vec![
            ("transport", "casper".to_owned()),
            (
                "command",
                format!("{} run {}", self.program, self.card.name),
            ),
        ]
    }

    fn name(&self) -> &str {
        &self.card.name
    }

    fn description(&self) -> &str {
        &self.card.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.card.parameters.clone()
    }

    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let mut call = Call {
            tool: self.card.name.clone(),
            args: arguments.clone(),
            cwd: ops.cwd().display().to_string(),
            answered: None,
        };
        // **magi decides, casper describes.** The card says which verb this tool acts under and
        // the ledger answers — the same ledger, the same prompt and the same standing grants
        // every tool here goes through. Without this a tool could be moved out of magi's config
        // and quietly leave the gate behind it, which is the one thing that must not happen
        // while tools are being moved.
        if let Some(action) = wants(&self.card, arguments)
            && let Err(why) = ops.allow(&self.card.name, &action)
        {
            return Output::error(why);
        }
        // **A call may stop and ask, and then go on.** Bounded, because a tool that asked
        // forever would hold the turn open forever: two questions is a permission and then a
        // confirmation, which is as far as anything has needed to go, and a third is a
        // declaration in a loop rather than one talking to a person.
        for _ in 0..3 {
            let ran = match run(&self.program, &call) {
                // A refusal is still something the model reads: it asked for a tool that could
                // not be reached, and the answer is to try another way rather than end the turn.
                Err(why) => return Output::error(why),
                Ok(ran) => ran,
            };
            // **Rows a tool fills itself.** The general form of a question: magi reserves the
            // space and drives the surface, and what goes in it is the tool's business. The
            // answer comes back as an id and resumes the call exactly as an answered question
            // does — one mechanism, so a picker, a permission and a game are one code path.
            if let Some(Shown::Surface(surface)) = &ran.shown {
                let Some(chosen) = self.holds.hold(&self.card.name, surface, arguments) else {
                    return Output::error(format!(
                        "{} wanted the screen for {} and there was none",
                        self.card.name, surface.about
                    ));
                };
                call.answered = Some(chosen);
                continue;
            }
            let Some(Shown::Ask(ask)) = &ran.shown else {
                return finished(ran);
            };
            let Some(choice) = self.asks.ask(&self.card.name, ask) else {
                // Nobody answered. Told to the model rather than left as an empty result: a
                // blank answer to a call it made reads as a tool that silently does nothing.
                return Output::error(format!(
                    "{} stopped to ask \"{}\" and nobody answered",
                    self.card.name, ask.question
                ));
            };
            call.answered = Some(choice);
        }
        Output::error(format!(
            "{} kept asking rather than answering",
            self.card.name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_casper_that_is_not_there_offers_nothing_rather_than_failing() {
        // The ordinary case on a machine without it. A session then keeps the tools magi
        // declares itself, exactly as it did before casper existed.
        assert!(cards_from("magi-no-such-casper-anywhere").is_empty());
    }

    #[test]
    fn a_call_to_something_absent_says_which_program() {
        let why = run(
            "magi-no-such-casper-anywhere",
            &Call {
                tool: "ls".to_owned(),
                args: serde_json::Value::Null,
                cwd: String::new(),
                answered: None,
            },
        )
        .expect_err("nothing to call");
        assert!(why.contains("magi-no-such-casper-anywhere"), "{why}");
    }

    #[test]
    fn a_reply_that_is_not_the_familys_shape_yields_nothing() {
        assert!(rows(b"not json at all").is_none());
        assert!(rows(br#"{"ok":false,"error":"no"}"#).is_none());
        assert_eq!(
            rows(br#"{"ok":true,"n":1,"result":[1]}"#).map(|r| r.len()),
            Some(1)
        );
    }
}

/// One finished call, as the registry wants it.
///
/// Both faces cross: `said` for the model, `shown` for the screen. A tool that reported a
/// problem is still a result — the model needs to read what went wrong in order to do something
/// about it — so a failure carries its view too.
fn finished(ran: Ran) -> Output {
    Output {
        content: ran.said,
        is_error: ran.failed,
        shown: ran.shown,
    }
}

/// What this call is about to do, in magi's own vocabulary.
///
/// `None` when the card names no verb, or names one this build has no meaning for: a tool that
/// touches nothing a person would want a say over is not gated, and a verb nobody recognises is
/// *not* silently treated as harmless — it is treated as `run`, which is the most guarded thing
/// there is. A newer casper inventing a verb should be asked about, not waved through.
fn wants(card: &Card, arguments: &serde_json::Value) -> Option<magi_proto::permit::Action> {
    use magi_proto::permit::Action;
    let needs = card.needs.as_deref()?;
    // The argument a person would judge it by, when there is an obvious one. A tool with no
    // path in its arguments is asked about by name, which is still better than not being asked.
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let path = || {
        let path = text("path");
        if path.is_empty() {
            card.name.clone()
        } else {
            path
        }
    };
    Some(match needs {
        "read" => Action::Read { path: path() },
        "write" => Action::Write { path: path() },
        "reach" => Action::Network { host: text("host") },
        _ => {
            let command = {
                let command = text("command");
                if command.is_empty() {
                    card.name.clone()
                } else {
                    command
                }
            };
            // The same reading the process transport uses. Two of them disagreed: this one took
            // the first word outright, so `FOO=1 git status` offered "any `FOO=1` command" — a
            // question nobody could answer sensibly, and one whose grant then covered every
            // command line starting `FOO=1`, including one that goes on to say something else
            // entirely. A permission subject that differs by transport is its own bug.
            let program = crate::process::first_word(&command);
            let program = if program.is_empty() {
                card.name.clone()
            } else {
                program
            };
            Action::Run { command, program }
        }
    })
}

/// What a card asks magi to decide before it runs.
#[cfg(test)]
mod gating {
    use super::*;
    use magi_proto::permit::Action;

    fn card(needs: Option<&str>) -> Card {
        Card {
            name: "bash".to_owned(),
            description: String::new(),
            parameters: serde_json::json!({}),
            needs: needs.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn a_tool_that_needs_nothing_is_not_gated() {
        // Asking about something nobody would want a say over is how a permission prompt
        // becomes a nuisance, and a nuisance is answered without being read.
        assert!(wants(&card(None), &serde_json::json!({})).is_none());
    }

    #[test]
    fn a_verb_becomes_the_action_magi_already_knows_how_to_ask_about() {
        let read = wants(&card(Some("read")), &serde_json::json!({"path": "/tmp/x"}));
        assert_eq!(
            read,
            Some(Action::Read {
                path: "/tmp/x".to_owned()
            })
        );
        let run = wants(
            &card(Some("run")),
            &serde_json::json!({"command": "rm -rf build"}),
        );
        assert_eq!(
            run,
            Some(Action::Run {
                command: "rm -rf build".to_owned(),
                program: "rm".to_owned(),
            })
        );
    }

    #[test]
    fn a_verb_nobody_recognises_is_guarded_rather_than_waved_through() {
        // A newer casper inventing a verb must not be treated as harmless: the safe reading of
        // "I do not know what this is" is the most guarded thing there is, not the least.
        let odd = wants(&card(Some("teleport")), &serde_json::json!({}));
        assert!(matches!(odd, Some(Action::Run { .. })), "{odd:?}");
    }

    #[test]
    fn a_tool_with_nothing_to_name_is_asked_about_by_its_own_name() {
        // Better than an empty prompt: "bash wants to run bash" is odd, and "bash wants to run"
        // with a blank where the command goes is worse.
        let bare = wants(&card(Some("read")), &serde_json::json!({}));
        assert_eq!(
            bare,
            Some(Action::Read {
                path: "bash".to_owned()
            })
        );
    }
}
