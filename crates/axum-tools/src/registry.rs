//! The one registry every tool lands in.

use crate::{Cancel, Ops, Output};
use std::collections::BTreeMap;

/// Something the model can call.
///
/// Implemented once per transport, never once per tool: `builtin` here, `lua` in `axum-lua`,
/// `process` over the wire. A caller holding a `&dyn Tool` cannot tell which it has.
///
/// Deliberately not `Send + Sync`. A Lua tool's body lives in a VM that is neither, and a
/// registry is built on -- and never leaves -- the worker thread that owns it. Demanding the
/// bounds would force an `unsafe impl` asserting exactly what the design already guarantees.
pub trait Tool {
    /// The name the model calls it by.
    fn name(&self) -> &str;

    /// What it does, in the model's terms.
    fn description(&self) -> &str;

    /// JSON Schema for its arguments.
    fn parameters(&self) -> serde_json::Value;

    /// Run it.
    ///
    /// Infallible on purpose: every failure a tool can have is something the model should read,
    /// so it comes back as [`Output::error`] rather than as an error the turn loop has to
    /// invent a message for.
    fn run(&self, arguments: &serde_json::Value, ops: &dyn Ops, cancel: &dyn Cancel) -> Output;

    /// Ask the tool to confirm what it offers, before the model is told about it.
    ///
    /// Nothing by default, because a tool written here is its own description and cannot
    /// disagree with itself. A peer can: it is another program, and what a config says about
    /// it is a claim rather than a fact.
    fn probe(&self, _ops: &dyn Ops) {}

    /// Start the call without waiting for it, if this tool can be waited on separately.
    ///
    /// The half of a round that overlaps. A model asking for three files, or a grep and an ls,
    /// used to pay for them one after another; sending all three and then collecting them costs
    /// the slowest rather than the sum.
    ///
    /// [`Sending::Inline`] by default, which means "there is nothing to overlap — call `run`".
    /// That is the honest answer for a built-in, whose work is a syscall, and for a Lua tool,
    /// whose work happens in a VM this thread owns. Only a peer has something to wait *for*:
    /// it is another process, and the waiting is what overlaps.
    ///
    /// Anything a tool refuses before sending — a permission it was denied — comes back here as
    /// [`Sending::Refused`], because a call that never went out has its answer already.
    fn send(&self, arguments: &serde_json::Value, ops: &dyn Ops) -> Sending {
        let _ = (arguments, ops);
        Sending::Inline
    }

    /// Wait for a call [`Tool::send`] started.
    ///
    /// Only called after `send` answered [`Sending::Sent`], so the default cannot be reached by
    /// a tool that did not opt in.
    fn wait(&self, cancel: &dyn Cancel) -> Output {
        let _ = cancel;
        Output::error("this tool was never sent")
    }
}

/// What [`Tool::send`] did with the call.
pub enum Sending {
    /// It is on its way. [`Tool::wait`] has the answer.
    Sent,
    /// This tool has nothing to overlap; call [`Tool::run`].
    Inline,
    /// It never went out, and this is why.
    Refused(Output),
}

/// Every tool the session can reach.
///
/// Keyed by name, so a later declaration replaces an earlier one and the name is the identity.
/// Tau needs per-instance prefixes because two instances of one extension both declare
/// `shell`; a keyed registrar makes that impossible to express in the first place.
#[derive(Default)]
pub struct Registry {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool, replacing any of the same name.
    ///
    /// Replacement rather than refusal because a config that declares `bash` means it: the
    /// point of shipping a default is that it can be overridden.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }

    /// Ask every tool to confirm what it offers.
    ///
    /// Called once, after the registry is built and before any turn: a schema the model was
    /// given cannot be corrected halfway through a conversation it has already used it in.
    pub fn probe(&self, ops: &dyn Ops) {
        for tool in self.tools.values() {
            tool.probe(ops);
        }
    }

    /// One tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(AsRef::as_ref)
    }

    /// How many tools are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Every tool, as a provider needs them declared.
    #[must_use]
    pub fn declarations(&self) -> Vec<axum_model::Tool> {
        self.tools
            .values()
            .map(|tool| axum_model::Tool {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters: tool.parameters(),
            })
            .collect()
    }

    /// Run a call, or say why it could not be run.
    ///
    /// An unknown tool is an [`Output::error`] rather than a hole: the model asked for
    /// something that does not exist and needs to be told, not left waiting.
    ///
    /// Every result is bounded here. This is the only point every transport passes through, and
    /// a tool cannot be trusted to cap itself: a peer is another program, and a Lua tool has no
    /// way to write the spill file that makes a cap survivable. See [`crate::bound`].
    #[must_use]
    pub fn call(
        &self,
        name: &str,
        arguments: &serde_json::Value,
        ops: &dyn Ops,
        cancel: &dyn Cancel,
    ) -> Output {
        match self.get(name) {
            Some(tool) => {
                // Checked here rather than by each tool, because this is where the schema is:
                // the tool published one and nothing ever read it back, so a call that did not
                // fit reached the tool anyway and failed in words about the tool's own internals
                // instead of about the call. The tool is not entered when it does not fit.
                let arguments = match crate::schema::check(arguments, &tool.parameters()) {
                    Ok(checked) => checked,
                    Err(wrong) => {
                        return Output::error(format!(
                            "{name}: the arguments do not fit:\n{wrong}"
                        ));
                    }
                };
                let output = tool.run(&arguments, ops, cancel);
                Output {
                    content: crate::bound::apply(name, output.content),
                    is_error: output.is_error,
                }
            }
            None => {
                let known: Vec<&str> = self.tools.keys().map(String::as_str).collect();
                Output::error(format!(
                    "there is no tool called {name:?}. Available: {}",
                    known.join(", ")
                ))
            }
        }
    }

    /// Answer a call whose arguments are still the text the model streamed.
    ///
    /// The entry point a turn uses, and the reason it exists: the raw text is the only place a
    /// repair can happen, and it was being thrown away. `call.parsed().unwrap_or(Value::Null)`
    /// handed the tool `null` **as if the model had asked for nothing** — no error, no retry, and
    /// nothing anywhere saying the arguments had not parsed, so the model sent the same broken
    /// call again. See [`crate::repair`] for what is mended and what is reported.
    #[must_use]
    pub fn answer(
        &self,
        name: &str,
        arguments: &str,
        ops: &dyn Ops,
        cancel: &dyn Cancel,
    ) -> Output {
        self.finish(self.prepare(name, arguments, ops), ops, cancel)
    }

    /// Check a call and start it, without waiting for the answer.
    ///
    /// The first half of a round, and it runs **one call at a time**: repairing, checking against
    /// the schema and asking the person for permission are all things that must happen in the
    /// order the model asked, and two permission prompts racing onto one screen is not a faster
    /// round but an unanswerable one. Pi draws the line in the same place — sequential
    /// preparation, parallel execution, results in source order (`agent-loop.ts:489-554`).
    ///
    /// What overlaps is the *waiting*, which is [`Registry::finish`].
    #[must_use]
    pub fn prepare(&self, name: &str, arguments: &str, ops: &dyn Ops) -> Prepared {
        let Some(tool) = self.get(name) else {
            let known: Vec<&str> = self.tools.keys().map(String::as_str).collect();
            return Prepared::answered(
                name,
                Output::error(format!(
                    "there is no tool called {name:?}. Available: {}",
                    known.join(", ")
                )),
            );
        };
        let parsed = match crate::repair::arguments(arguments) {
            Ok(parsed) => parsed,
            Err(why) => return Prepared::answered(name, Output::error(format!("{name}: {why}"))),
        };
        let checked = match crate::schema::check(&parsed, &tool.parameters()) {
            Ok(checked) => checked,
            Err(wrong) => {
                return Prepared::answered(
                    name,
                    Output::error(format!("{name}: the arguments do not fit:\n{wrong}")),
                );
            }
        };
        let state = match tool.send(&checked, ops) {
            Sending::Sent => State::Sent,
            Sending::Inline => State::Inline(checked),
            Sending::Refused(output) => State::Answered(output),
        };
        Prepared {
            name: name.to_owned(),
            state,
        }
    }

    /// Collect what [`Registry::prepare`] started.
    ///
    /// The second half, and the half that overlaps: a call already sent is only waited on here,
    /// so a round of calls to different peers costs the slowest rather than the sum. Called in
    /// the order the model asked, so the transcript is the same whatever the timing was.
    #[must_use]
    pub fn finish(&self, prepared: Prepared, ops: &dyn Ops, cancel: &dyn Cancel) -> Output {
        let Prepared { name, state } = prepared;
        let output = match state {
            State::Answered(output) => return output,
            State::Sent => match self.get(&name) {
                Some(tool) => tool.wait(cancel),
                None => Output::error(format!("{name} is gone")),
            },
            State::Inline(arguments) => match self.get(&name) {
                Some(tool) => tool.run(&arguments, ops, cancel),
                None => Output::error(format!("{name} is gone")),
            },
        };
        Output {
            content: crate::bound::apply(&name, output.content),
            is_error: output.is_error,
        }
    }
}

/// A call that has been checked, and started if it could be.
pub struct Prepared {
    name: String,
    state: State,
}

impl Prepared {
    fn answered(name: &str, output: Output) -> Self {
        Self {
            name: name.to_owned(),
            state: State::Answered(output),
        }
    }

    /// Whether this call is already out and only needs collecting.
    ///
    /// What makes a round worth splitting: nothing is overlapping unless something is in flight.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        matches!(self.state, State::Sent)
    }
}

/// How far a prepared call got.
enum State {
    /// Sent to a peer, waiting to be collected.
    Sent,
    /// Not sent; run it where it stands, with these arguments.
    Inline(serde_json::Value),
    /// It never started, and this is why.
    Answered(Output),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Real;

    struct Fake(&'static str);

    impl Tool for Fake {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "a fake"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn run(
            &self,
            _arguments: &serde_json::Value,
            _ops: &dyn Ops,
            _cancel: &dyn Cancel,
        ) -> Output {
            Output::ok(self.0)
        }
    }

    fn ops() -> Real {
        Real::new(std::env::temp_dir())
    }

    #[test]
    fn a_registered_tool_can_be_called() {
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("read")));
        let output = registry.call("read", &serde_json::json!({}), &ops(), &crate::Uncancelled);
        assert_eq!(output.content, "read");
        assert!(!output.is_error);
    }

    #[test]
    fn a_later_declaration_replaces_an_earlier_one() {
        // The point of shipping a default is that a config can override it.
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("bash")));
        registry.register(Box::new(Fake("bash")));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn an_unknown_tool_is_told_so_and_shown_what_exists() {
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("read")));
        let output = registry.call("nope", &serde_json::json!({}), &ops(), &crate::Uncancelled);
        assert!(output.is_error);
        assert!(output.content.contains("nope"), "{}", output.content);
        assert!(output.content.contains("read"), "{}", output.content);
    }

    #[test]
    fn declarations_carry_what_a_provider_needs() {
        let mut registry = Registry::new();
        registry.register(Box::new(Fake("read")));
        let declared = registry.declarations();
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].name, "read");
        assert_eq!(declared[0].parameters["type"], "object");
    }

    #[test]
    fn an_empty_registry_says_so() {
        assert!(Registry::new().is_empty());
    }
}

#[cfg(test)]
mod bound_tests {
    use super::*;
    use crate::cancel::Uncancelled;

    struct Flood;

    impl Tool for Flood {
        fn name(&self) -> &str {
            "flood"
        }
        fn description(&self) -> &str {
            "returns more than anyone asked for"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        fn run(&self, _: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
            Output::ok(
                (1..=50_000)
                    .map(|i| format!("line {i}\n"))
                    .collect::<String>(),
            )
        }
    }

    #[test]
    fn a_flood_is_capped_before_it_reaches_the_caller() {
        // The cap is here and not in the tool, because a peer is another program and a Lua tool
        // cannot write a spill file. One `cat` of a lockfile used to be permanent: journalled,
        // replayed on every request, inside the tail compaction keeps verbatim, and then fed to
        // the summariser.
        let mut registry = Registry::new();
        registry.register(Box::new(Flood));
        let ops = crate::ops::Real::new(std::env::temp_dir());
        let out = registry.call("flood", &serde_json::json!({}), &ops, &Uncancelled);
        assert!(out.content.len() < 200_000, "{} bytes", out.content.len());
        assert!(
            out.content.contains("cut from the middle"),
            "and it says so"
        );
    }

    #[test]
    fn a_small_result_is_passed_through_unchanged() {
        struct Quiet;
        impl Tool for Quiet {
            fn name(&self) -> &str {
                "quiet"
            }
            fn description(&self) -> &str {
                ""
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({ "type": "object" })
            }
            fn run(&self, _: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
                Output::ok("ok")
            }
        }
        let mut registry = Registry::new();
        registry.register(Box::new(Quiet));
        let ops = crate::ops::Real::new(std::env::temp_dir());
        let out = registry.call("quiet", &serde_json::json!({}), &ops, &Uncancelled);
        assert_eq!(out.content, "ok");
    }

    #[test]
    fn an_error_result_is_still_bounded_and_still_an_error() {
        struct Loud;
        impl Tool for Loud {
            fn name(&self) -> &str {
                "loud"
            }
            fn description(&self) -> &str {
                ""
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({ "type": "object" })
            }
            fn run(&self, _: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
                Output::error(
                    (1..=50_000)
                        .map(|i| format!("bad {i}\n"))
                        .collect::<String>(),
                )
            }
        }
        let mut registry = Registry::new();
        registry.register(Box::new(Loud));
        let ops = crate::ops::Real::new(std::env::temp_dir());
        let out = registry.call("loud", &serde_json::json!({}), &ops, &Uncancelled);
        assert!(out.is_error, "a failure that is long is still a failure");
        assert!(out.content.len() < 200_000);
    }
}

#[cfg(test)]
mod checked_tests {
    use super::*;
    use crate::ops::Real;
    use std::cell::Cell;

    /// A tool that records whether it was entered.
    struct Counted {
        ran: std::rc::Rc<Cell<usize>>,
    }

    impl Tool for Counted {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            "reads a file"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 },
                },
                "required": ["path"],
            })
        }
        fn run(&self, arguments: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
            self.ran.set(self.ran.get() + 1);
            Output::ok(arguments.to_string())
        }
    }

    fn counted() -> (Registry, Real, std::rc::Rc<Cell<usize>>) {
        let ran = std::rc::Rc::new(Cell::new(0));
        let mut registry = Registry::new();
        registry.register(Box::new(Counted {
            ran: std::rc::Rc::clone(&ran),
        }));
        (registry, Real::new(std::env::temp_dir()), ran)
    }

    #[test]
    fn a_call_that_does_not_fit_never_reaches_the_tool() {
        // The whole point. It used to reach the tool, which failed for its own reasons in its
        // own words, and nothing said the call had been wrong.
        let (registry, ops, ran) = counted();
        let out = registry.call("read", &serde_json::json!({}), &ops, &crate::Uncancelled);
        assert!(out.is_error);
        assert!(out.content.contains("path: required"), "{}", out.content);
        assert_eq!(ran.get(), 0, "the tool was not entered");
    }

    #[test]
    fn malformed_json_comes_back_naming_the_problem_rather_than_as_null() {
        // The defect this milestone is about: `parsed().unwrap_or(Null)` handed the tool
        // `null`, which reads as "the model asked for nothing", and it failed for a reason
        // that had nothing to do with the mistake.
        let (registry, ops, ran) = counted();
        let out = registry.answer("read", r#"{"path": "a.rs"#, &ops, &crate::Uncancelled);
        assert!(out.is_error);
        assert!(out.content.contains("not valid JSON"), "{}", out.content);
        assert_eq!(ran.get(), 0, "the tool was not entered");
    }

    #[test]
    fn a_raw_newline_is_repaired_and_the_call_goes_through() {
        // Between the two: not valid JSON, and not the model's meaning being unclear either.
        let (registry, ops, ran) = counted();
        let out = registry.answer("read", "{\"path\":\"a\nb.rs\"}", &ops, &crate::Uncancelled);
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(ran.get(), 1, "the tool ran");
        assert!(out.content.contains("a\\nb.rs"), "{}", out.content);
    }

    #[test]
    fn the_tool_receives_the_coerced_arguments_rather_than_what_arrived() {
        // A provider stringified the number; the tool should never have to know that happened.
        let (registry, ops, ran) = counted();
        let out = registry.answer(
            "read",
            r#"{"path":"a.rs","limit":"5"}"#,
            &ops,
            &crate::Uncancelled,
        );
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(ran.get(), 1);
        assert!(out.content.contains("\"limit\":5"), "{}", out.content);
    }

    #[test]
    fn a_call_with_no_arguments_at_all_is_an_empty_object() {
        // Providers differ on whether they send `{}` or nothing for a no-argument call.
        let (registry, ops, _) = counted();
        let out = registry.answer("read", "", &ops, &crate::Uncancelled);
        assert!(out.is_error, "the schema still requires a path");
        assert!(out.content.contains("path: required"), "{}", out.content);
    }

    #[test]
    fn an_unknown_tool_is_still_answered_before_anything_is_checked() {
        // There is no schema to check against, and the model needs the list either way.
        let (registry, ops, _) = counted();
        let out = registry.answer("nope", "{}", &ops, &crate::Uncancelled);
        assert!(out.is_error);
        assert!(out.content.contains("no tool called"), "{}", out.content);
    }
}
