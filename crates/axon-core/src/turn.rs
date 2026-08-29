//! One turn: provider deltas in, a decision out.

use axon_model::{Content, Message, Role, StopReason, Usage};
use axon_provider::api::Delta;

/// What a turn is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnState {
    /// Nothing in flight.
    Idle,
    /// A response is arriving.
    Streaming,
    /// The model asked for tools and is waiting on their results.
    ToolsPending,
    /// The turn is over.
    Finished(StopReason),
}

/// What the driver should do next.
///
/// Returned rather than performed: the state machine decides, the daemon acts. That split is
/// what keeps this crate free of I/O and the daemon free of agent logic.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Send the conversation to the provider.
    CallProvider,
    /// Run these, then hand the results back.
    RunTools(Vec<PendingCall>),
    /// Nothing more to do.
    Done(StopReason),
}

/// A tool the model asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingCall {
    /// Provider-issued identity, which the result must quote back.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as the model streamed them, still raw JSON text.
    pub arguments: String,
}

impl PendingCall {
    /// The arguments, parsed.
    ///
    /// # Errors
    /// When the model produced JSON that does not parse — which a truncated turn will.
    pub fn parsed(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_str(&self.arguments)
    }
}

/// A turn in progress.
#[derive(Debug, Clone, Default)]
pub struct Turn {
    state: State,
    text: String,
    thinking: String,
    signature: Option<String>,
    calls: Vec<PendingCall>,
    usage: Usage,
    stop: Option<StopReason>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    Streaming,
    Finished,
}

impl Turn {
    /// A turn that has not started.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What this turn is doing.
    #[must_use]
    pub fn state(&self) -> TurnState {
        match (self.state, self.stop) {
            (State::Idle, _) => TurnState::Idle,
            (State::Streaming, _) => TurnState::Streaming,
            (State::Finished, Some(StopReason::ToolUse)) if !self.calls.is_empty() => {
                TurnState::ToolsPending
            }
            (State::Finished, stop) => TurnState::Finished(stop.unwrap_or(StopReason::EndTurn)),
        }
    }

    /// Response text so far.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The opaque state the provider issued for this turn's reasoning.
    ///
    /// Handed straight back on the next request. A provider that checks — Anthropic, for
    /// extended thinking with tools — rejects a turn that continues without it.
    #[must_use]
    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    /// Reasoning so far.
    #[must_use]
    pub fn thinking(&self) -> &str {
        &self.thinking
    }

    /// Tokens consumed.
    #[must_use]
    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// Fold one provider delta into the turn.
    pub fn apply(&mut self, delta: Delta) {
        if self.state == State::Idle {
            self.state = State::Streaming;
        }
        match delta {
            Delta::Text(text) => self.text.push_str(&text),
            Delta::Thinking(text) => self.thinking.push_str(&text),
            Delta::Signature(signature) => self.signature = Some(signature),
            Delta::ToolCallStart { id, name } => self.calls.push(PendingCall {
                id,
                name,
                arguments: String::new(),
            }),
            // Arguments belong to the call that opened most recently; a provider streams one
            // call's arguments to completion before opening the next.
            Delta::ToolCallArgs(chunk) => {
                if let Some(call) = self.calls.last_mut() {
                    call.arguments.push_str(&chunk);
                }
            }
            Delta::Usage(usage) => self.usage = usage,
            Delta::Stop(reason) => {
                self.stop = Some(reason);
                self.state = State::Finished;
            }
        }
    }

    /// End the turn early, as an interrupt or a transport failure does.
    pub fn abort(&mut self, reason: StopReason) {
        self.stop = Some(reason);
        self.state = State::Finished;
    }

    /// What the driver should do next.
    #[must_use]
    pub fn step(&self) -> Step {
        match self.state() {
            TurnState::Idle | TurnState::Streaming => Step::CallProvider,
            TurnState::ToolsPending => Step::RunTools(self.calls.clone()),
            TurnState::Finished(reason) => Step::Done(reason),
        }
    }

    /// The assistant message this turn produced.
    #[must_use]
    pub fn message(&self) -> Message {
        let mut content = Vec::new();
        if !self.thinking.is_empty() {
            content.push(Content::Thinking {
                thinking: self.thinking.clone(),
                signature: self.signature.clone(),
            });
        }
        if !self.text.is_empty() {
            content.push(Content::Text {
                text: self.text.clone(),
                signature: None,
            });
        }
        for call in &self.calls {
            content.push(Content::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                // Arguments that did not finish arriving are kept as they are rather than
                // dropped: the transcript should show what the model actually produced. As the
                // text, because `null` is a different call — it says the model asked for
                // nothing, and what it did was ask for something and get cut off.
                arguments: call
                    .parsed()
                    .unwrap_or_else(|_| serde_json::Value::String(call.arguments.clone())),
                thought_signature: None,
            });
        }
        Message {
            role: Role::Assistant,
            content,
            stop_reason: self.stop,
            usage: Some(self.usage),
            error: None,
        }
    }

    /// The results a truncated turn must produce instead of running its tools.
    ///
    /// A `length` stop can land mid-arguments, and truncated JSON can still parse into
    /// something schema-valid — so every call in the turn is failed rather than any of them
    /// being run. Pi solved this the same way and it is a real bug class, not a nicety.
    #[must_use]
    pub fn poisoned_results(&self) -> Vec<Message> {
        if self.stop != Some(StopReason::Length) {
            return Vec::new();
        }
        self.calls
            .iter()
            .map(|call| Message {
                role: Role::Tool,
                content: vec![Content::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    content: "The response was truncated before this call was complete. \
                              Re-issue it with complete arguments."
                        .to_owned(),
                    is_error: true,
                }],
                stop_reason: None,
                usage: None,
                error: None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fold a sequence of deltas into a turn.
    fn turn(deltas: Vec<Delta>) -> Turn {
        let mut turn = Turn::new();
        for delta in deltas {
            turn.apply(delta);
        }
        turn
    }

    fn call(id: &str, name: &str) -> Delta {
        Delta::ToolCallStart {
            id: id.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn a_fresh_turn_asks_for_the_provider() {
        assert_eq!(Turn::new().state(), TurnState::Idle);
        assert_eq!(Turn::new().step(), Step::CallProvider);
    }

    #[test]
    fn text_accumulates_while_streaming() {
        let turn = turn(vec![Delta::Text("Hel".into()), Delta::Text("lo".into())]);
        assert_eq!(turn.text(), "Hello");
        assert_eq!(turn.state(), TurnState::Streaming);
    }

    #[test]
    fn a_finished_turn_is_done() {
        let turn = turn(vec![
            Delta::Text("hi".into()),
            Delta::Stop(StopReason::EndTurn),
        ]);
        assert_eq!(turn.state(), TurnState::Finished(StopReason::EndTurn));
        assert_eq!(turn.step(), Step::Done(StopReason::EndTurn));
    }

    #[test]
    fn a_turn_that_asked_for_tools_waits_on_them() {
        let turn = turn(vec![
            call("t1", "read"),
            Delta::ToolCallArgs(r#"{"path":"a"}"#.into()),
            Delta::Stop(StopReason::ToolUse),
        ]);
        assert_eq!(turn.state(), TurnState::ToolsPending);
        let Step::RunTools(calls) = turn.step() else {
            panic!("expected tools, got {:?}", turn.step());
        };
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].parsed().expect("json")["path"], "a");
    }

    #[test]
    fn arguments_land_on_the_call_that_opened_most_recently() {
        let turn = turn(vec![
            call("t1", "read"),
            Delta::ToolCallArgs(r#"{"a":1}"#.into()),
            call("t2", "write"),
            Delta::ToolCallArgs(r#"{"b":2}"#.into()),
            Delta::Stop(StopReason::ToolUse),
        ]);
        let Step::RunTools(calls) = turn.step() else {
            panic!("expected tools");
        };
        assert_eq!(calls[0].parsed().expect("json")["a"], 1);
        assert_eq!(calls[1].parsed().expect("json")["b"], 2);
    }

    #[test]
    fn a_tool_use_stop_with_no_calls_is_simply_finished() {
        // A provider that says `tool_use` and streams none has ended the turn, not started a
        // wait that nothing will ever satisfy.
        let turn = turn(vec![Delta::Stop(StopReason::ToolUse)]);
        assert_eq!(turn.state(), TurnState::Finished(StopReason::ToolUse));
    }

    #[test]
    fn a_truncated_turn_fails_every_call_rather_than_running_any() {
        // Truncated JSON can still parse into something schema-valid, so a `length` stop
        // poisons the whole turn.
        let turn = turn(vec![
            call("t1", "read"),
            Delta::ToolCallArgs(r#"{"path":"a"}"#.into()),
            call("t2", "write"),
            Delta::ToolCallArgs(r#"{"path":"#.into()),
            Delta::Stop(StopReason::Length),
        ]);
        let poisoned = turn.poisoned_results();
        assert_eq!(poisoned.len(), 2, "both calls, not just the broken one");
        for message in &poisoned {
            let Content::ToolResult {
                is_error, content, ..
            } = &message.content[0]
            else {
                panic!("expected a tool result");
            };
            assert!(is_error);
            assert!(content.contains("Re-issue"), "{content}");
        }
    }

    #[test]
    fn a_turn_that_finished_normally_poisons_nothing() {
        let turn = turn(vec![call("t1", "read"), Delta::Stop(StopReason::ToolUse)]);
        assert!(turn.poisoned_results().is_empty());
    }

    #[test]
    fn thinking_keeps_its_signature() {
        let turn = turn(vec![
            Delta::Thinking("why".into()),
            Delta::Signature("sig".into()),
            Delta::Stop(StopReason::EndTurn),
        ]);
        let message = turn.message();
        let Content::Thinking {
            thinking,
            signature,
        } = &message.content[0]
        else {
            panic!("expected thinking first");
        };
        assert_eq!(thinking, "why");
        assert_eq!(signature.as_deref(), Some("sig"));
    }

    #[test]
    fn the_message_orders_thinking_before_text() {
        let turn = turn(vec![
            Delta::Text("answer".into()),
            Delta::Thinking("reasoning".into()),
            Delta::Stop(StopReason::EndTurn),
        ]);
        let message = turn.message();
        assert!(matches!(message.content[0], Content::Thinking { .. }));
        assert!(matches!(message.content[1], Content::Text { .. }));
    }

    #[test]
    fn an_empty_turn_produces_a_message_with_no_blocks() {
        let turn = turn(vec![Delta::Stop(StopReason::EndTurn)]);
        assert!(turn.message().content.is_empty());
    }

    #[test]
    fn unparseable_arguments_still_appear_in_the_transcript() {
        // What the model actually produced is what should be shown, even when it is broken.
        let turn = turn(vec![
            call("t1", "read"),
            Delta::ToolCallArgs("{not json".into()),
            Delta::Stop(StopReason::ToolUse),
        ]);
        let message = turn.message();
        assert!(matches!(message.content[0], Content::ToolCall { .. }));
    }

    #[test]
    fn usage_is_carried_onto_the_message() {
        let turn = turn(vec![
            Delta::Usage(Usage {
                input: 10,
                output: 5,
                ..Usage::default()
            }),
            Delta::Stop(StopReason::EndTurn),
        ]);
        assert_eq!(turn.usage().input, 10);
        assert_eq!(turn.message().usage.expect("usage").output, 5);
    }

    #[test]
    fn aborting_finishes_the_turn_with_what_had_arrived() {
        let mut turn = turn(vec![Delta::Text("partial".into())]);
        turn.abort(StopReason::Aborted);
        assert_eq!(turn.state(), TurnState::Finished(StopReason::Aborted));
        assert_eq!(
            turn.text(),
            "partial",
            "an interrupted turn keeps its output"
        );
    }
}
