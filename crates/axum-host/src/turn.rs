//! Running one turn against a provider.
//!
//! The daemon's half of the loop: it owns the socket, the journal and the clock, and holds no
//! agent logic — [`axum_core::Turn`] decides what happens and this drives it.

use crate::session::Session;
use axum_core::{Step, Turn};
use axum_model::{Content, Context, Message, Role, StopReason};
use axum_proto::{AgentStatus, Entry, MessageId, ToolCallId};
use axum_provider::api::Options;
use axum_provider::client::Client;
use axum_provider::model::Model;
use axum_provider::provider::Provider;
use axum_tools::{Ops, Registry};

/// What the daemon needs to reach a model.
///
/// Plain data, and sendable: the protocol it names is built on the worker's own thread, because
/// a Lua VM is neither `Send` nor `Sync` and cannot be handed over after the fact.
#[derive(Debug, Clone)]
pub struct Backend {
    /// Tool descriptions to run in the VM, as `(name, source)`.
    pub tools: Vec<(String, String)>,
    /// The family's client stubs, so a Lua tool can talk to a sibling.
    pub stubs: Vec<(String, String)>,
    /// Where the session is rooted, which is what tools resolve paths against.
    pub cwd: std::path::PathBuf,
    /// The protocol descriptions to build the VM from, as `(name, source)`.
    ///
    /// Carried as text rather than as a built VM because a VM cannot cross a thread boundary,
    /// and read from the same place the catalog was so that a protocol the user edited is the
    /// one the daemon speaks. Building from the compiled-in copies instead was a bug: an
    /// edited `apis/*.lua` changed what `axum models` reported and nothing else.
    pub apis: Vec<(String, String)>,
    /// The provider offering the model.
    pub provider: Provider,
    /// The model to call.
    pub model: Model,
    /// What to ask for beyond the conversation.
    pub options: Options,
}

/// Summarise the earlier part of the conversation and journal the result.
///
/// Returns whether anything was compacted. A failure is not fatal: the turn goes ahead with
/// the context it has and either fits or is refused by the provider, which is no worse than
/// not having tried. Losing the conversation because the summariser had a bad minute would be.
async fn compact(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    adapter: &dyn axum_provider::api::Adapter,
    client: &Client,
) -> bool {
    let (context, entries) = {
        let held = session.lock().await;
        (context_of(&held), held.entries().len())
    };
    let Some(covered) = crate::compact::covers(entries) else {
        return false;
    };

    {
        let mut held = session.lock().await;
        held.set_status(AgentStatus::Working {
            label: "Compacting".into(),
        });
    }

    // Everything before the kept tail, in messages rather than entries: one entry can be
    // several messages, so the summariser is given what the provider would have been given.
    let through = context.messages.len().saturating_sub(crate::compact::KEEP);
    let asked = crate::compact::request(&context, through);
    let mut turn = axum_core::Turn::new();
    let mut deltas = Vec::new();
    let outcome = client
        .stream(
            adapter,
            &backend.provider,
            &backend.model,
            &asked,
            &backend.options,
            |delta| deltas.push(delta),
        )
        .await;
    for delta in deltas {
        turn.apply(delta);
    }
    if outcome.is_err() || turn.text().trim().is_empty() {
        return false;
    }

    let mut held = session.lock().await;
    let id = MessageId::new(format!("k{}", held.cursor().next().0));
    let committed = held.commit(Entry::Compaction {
        id,
        summary: turn.text().trim().to_owned(),
        replaces: covered,
    });
    committed.is_ok()
}

/// Run one turn and journal what it produced.
///
/// Deltas are published as they arrive and the entry is amended as it grows, so a UI attaching
/// mid-turn sees the same partial message a UI that was there all along sees. A crash leaves
/// the partial message rather than nothing, which is why the entry is written before it ends.
async fn one_turn(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    adapter: &dyn axum_provider::api::Adapter,
    client: &Client,
    tools: Vec<axum_model::Tool>,
    cancel: &crate::cancel::Cancel,
) -> Result<Round, crate::HostError> {
    let mut context = context_of(&*session.lock().await);
    context.tools = tools;

    {
        let mut held = session.lock().await;
        held.set_status(AgentStatus::Working {
            label: "Thinking".into(),
        });
    }

    let id = MessageId::new(format!("a{}", session.lock().await.cursor().next().0));
    let mut turn = Turn::new();

    // The entry exists before the first delta, so a UI attaching mid-turn has something to
    // extend rather than a message that appears fully formed at the end.
    session.lock().await.commit(assistant(&id, &turn))?;

    let mut deltas = Vec::new();
    // The provider call is raced against the interrupt rather than polled after it: a model
    // mid-answer holds this future for as long as it keeps talking, and a flag checked when it
    // returns is a stop that arrives once the work it was stopping is already paid for.
    let outcome = tokio::select! {
        biased;
        () = cancel.requested() => Ok(()),
        outcome = client.stream(
            adapter,
            &backend.provider,
            &backend.model,
            &context,
            &backend.options,
            |delta| deltas.push(delta),
        ) => outcome,
    };

    for delta in deltas {
        turn.apply(delta);
    }

    // Whatever arrived before the interrupt is kept: the model said it, and a transcript that
    // drops a half-finished answer leaves the next prompt with no account of what happened.
    if cancel.is_requested() {
        turn.abort(StopReason::Aborted);
        let mut held = session.lock().await;
        held.amend(Entry::Assistant {
            id,
            text: turn.text().to_owned(),
            thinking: turn.thinking().to_owned(),
            stop_reason: Some(StopReason::Aborted),
            error: None,
            signatures: axum_proto::Signatures {
                text: None,
                thinking: turn.signature().map(str::to_owned),
            },
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(Round { turn, failed: None });
    }

    if let Err(error) = outcome {
        // An error is a value, not an exception: the transcript stays well-formed and the UI
        // needs no error branch. Pi's discipline, and the reason its renderer has none.
        let class = error.class;
        turn.abort(StopReason::Error);
        let mut held = session.lock().await;
        held.amend(Entry::Assistant {
            id,
            text: turn.text().to_owned(),
            thinking: turn.thinking().to_owned(),
            stop_reason: Some(StopReason::Error),
            error: Some(error.message),
            signatures: axum_proto::Signatures::default(),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(Round {
            turn,
            failed: Some(class),
        });
    }

    let mut held = session.lock().await;

    held.amend(assistant(&id, &turn))?;
    held.set_status(AgentStatus::Idle);
    Ok(Round { turn, failed: None })
}

/// What one round produced.
///
/// The turn on its own cannot say *why* it stopped: a failure becomes an error entry and a
/// sentence, and by then the class that would tell the loop whether to act is gone. This
/// carries it back, so an overflow can be answered by compacting instead of by giving up.
struct Round {
    turn: Turn,
    /// Set when the provider refused, and the class it refused with.
    failed: Option<axum_provider::retry::RetryClass>,
}

/// The assistant entry for a turn in its current state.
fn assistant(id: &MessageId, turn: &Turn) -> Entry {
    Entry::Assistant {
        id: id.clone(),
        text: turn.text().to_owned(),
        thinking: turn.thinking().to_owned(),
        // Carried into the journal, because the journal is what the next request is built
        // from. Captured by the turn and then dropped here was the whole of the bug.
        signatures: axum_proto::Signatures {
            text: None,
            thinking: turn.signature().map(str::to_owned),
        },
        stop_reason: match turn.state() {
            axum_core::TurnState::Finished(reason) => Some(reason),
            _ => None,
        },
        error: None,
    }
}

/// Build the provider-facing conversation from the transcript.
///
/// The journal holds what was shown; a provider needs what was said. Tool entries become tool
/// results, and an assistant entry that failed is dropped — replaying an error as if the model
/// had said it teaches it to produce more of them.
pub fn context_of(session: &Session) -> Context {
    let entries = session.entries();
    // Where the conversation starts as far as the provider is concerned. The entries before it
    // are still in the journal and still on screen; what changed is what fits in the window.
    //
    // `replaces`, not the position of the record. A compaction is appended after the entries
    // it keeps, so starting from the record itself would skip the recent tail — the part it
    // went to the trouble of not summarising.
    let (from, summary) = entries
        .iter()
        .rev()
        .find_map(|e| match e {
            Entry::Compaction {
                summary, replaces, ..
            } => Some((*replaces, Some(summary.clone()))),
            _ => None,
        })
        .unwrap_or((0, None));

    let mut messages: Vec<Message> = Vec::new();
    if let Some(summary) = summary {
        // As a user message, because it is context the model is being given rather than
        // something it said. A model shown its own words as a summary tends to continue them.
        messages.push(Message::user(format!(
            "Here is a summary of the earlier part of this conversation:\n\n{summary}"
        )));
    }
    // Where the assistant message currently being rebuilt lives, so the tool entries that
    // follow it can put their calls back into it. The journal stores a call as its own record
    // -- it is committed before the registry is consulted, which is what makes an unrouted
    // call auditable -- but a provider needs it inside the message that made it.
    let mut open: Option<usize> = None;

    for entry in entries.iter().skip(from) {
        match entry {
            // A compaction inside the replayed range cannot happen: `from` is past the last
            // one. Ignored rather than handled, so adding a second kind of marker later is a
            // new arm and not a change to this one.
            Entry::Compaction { .. } => {}
            Entry::User { text, .. } => {
                open = None;
                messages.push(Message::user(text.clone()));
            }
            Entry::Assistant {
                text,
                thinking,
                stop_reason,
                error,
                signatures,
                ..
            } => {
                open = None;
                // Replaying an error as if the model had said it teaches it to produce more.
                if error.is_some() || *stop_reason == Some(StopReason::Error) {
                    continue;
                }
                let mut content = Vec::new();
                if !thinking.is_empty() {
                    content.push(Content::Thinking {
                        thinking: thinking.clone(),
                        signature: signatures.thinking.clone(),
                    });
                }
                if !text.is_empty() {
                    content.push(Content::Text {
                        text: text.clone(),
                        signature: signatures.text.clone(),
                    });
                }
                // Pushed even when empty, because the common shape of a tool-using turn is a
                // model that says nothing and calls something. The empty ones are pruned below.
                messages.push(Message {
                    role: Role::Assistant,
                    content,
                    stop_reason: *stop_reason,
                    usage: None,
                    error: None,
                });
                open = Some(messages.len() - 1);
            }
            Entry::Tool {
                id,
                name,
                args,
                result,
                thought_signature,
            } => {
                if let Some(at) = open {
                    messages[at].content.push(Content::ToolCall {
                        id: id.to_string(),
                        name: name.clone(),
                        arguments: serde_json::from_str(args).unwrap_or(serde_json::Value::Null),
                        thought_signature: thought_signature.clone(),
                    });
                }
                if let Some(result) = result {
                    messages.push(Message {
                        role: Role::Tool,
                        content: vec![Content::ToolResult {
                            id: id.to_string(),
                            name: name.clone(),
                            content: result.output.clone(),
                            is_error: result.is_error,
                        }],
                        stop_reason: None,
                        usage: None,
                        error: None,
                    });
                }
            }
        }
    }

    // The entry committed before the first delta has no content and never gained a call. A
    // message with nothing in it is rejected by every provider that checks.
    messages.retain(|m| !(m.role == Role::Assistant && m.content.is_empty()));
    Context {
        messages,
        ..Context::default()
    }
}

/// Rounds of tool use one prompt may take before the loop gives up.
///
/// A model that keeps asking for tools without finishing is not making progress, and an
/// unbounded loop spends money proving it. High enough that real work never reaches it.
const MAX_ROUNDS: usize = 24;

/// Run a prompt to completion: provider, tools, provider, until the turn ends.
///
/// Tools run between turns rather than during one, because a provider's answer is what says
/// which tools to run. Every result is journalled as its own entry, so the transcript shows
/// what was asked and what came back rather than only the conclusion.
pub async fn run(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    adapter: &dyn axum_provider::api::Adapter,
    client: &Client,
    registry: &Registry,
    ops: &dyn Ops,
) -> Result<(), crate::HostError> {
    // Taken once: the handle is a clone of shared state, so a stop asked for mid-round is
    // visible through it without going back to the session for a fresh one.
    let cancel = session.lock().await.cancel();

    // Before the first round, not before every one: a turn adds at most a few messages, and
    // compacting between rounds of one prompt would summarise a conversation the model is
    // still in the middle of.
    let over = {
        let held = session.lock().await;
        crate::compact::needed(&context_of(&held), &backend.model)
    };
    if over {
        compact(session, backend, adapter, client).await;
    }

    // One reactive compaction per prompt. A second overflow after summarising is not a
    // conversation that is too long -- it is one whose kept tail alone will not fit, and
    // compacting again would summarise the summary and still fail.
    let mut compacted = false;

    for _ in 0..MAX_ROUNDS {
        let round = one_turn(
            session,
            backend,
            adapter,
            client,
            registry.declarations(),
            &cancel,
        )
        .await?;

        // The estimate above is deliberately rough; this is the provider's own answer. The
        // failed round stays in the transcript, because a reader who notices the model
        // forgetting something deserves to see that this is why.
        if round.failed == Some(axum_provider::retry::RetryClass::Overflow) && !compacted {
            compacted = true;
            if compact(session, backend, adapter, client).await {
                continue;
            }
        }
        let turn = round.turn;

        // An interrupted turn has already been journalled as aborted; continuing would call the
        // provider again with the stop still pending and abort that one too.
        if cancel.is_requested() {
            return Ok(());
        }

        // A truncated turn poisons its own calls: `length` can land mid-arguments, and
        // truncated JSON can still parse into something schema-valid.
        let poisoned = turn.poisoned_results();
        if !poisoned.is_empty() {
            let mut held = session.lock().await;
            for (call, _) in turn_calls(&turn).into_iter().zip(poisoned) {
                held.commit(Entry::Tool {
                    id: ToolCallId::new(call.id.clone()),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                    // No protocol description emits one yet. The slot is here because the
                    // journal is what the next request is rebuilt from, and a journal that
                    // cannot hold what the model layer holds is lossy by construction.
                    thought_signature: None,
                    result: Some(axum_proto::ToolResult {
                        output: "The response was truncated before this call was complete. \
                                 Re-issue it with complete arguments."
                            .to_owned(),
                        is_error: true,
                    }),
                })?;
            }
            held.set_status(AgentStatus::Idle);
            return Ok(());
        }

        let calls = turn_calls(&turn);
        if calls.is_empty() {
            return Ok(());
        }

        {
            let mut held = session.lock().await;
            held.set_status(AgentStatus::Working {
                label: "Running tools".into(),
            });
            for call in &calls {
                // Journalled before it is run, and before the registry is consulted: a call
                // that went nowhere is still something the transcript can account for. Tau
                // calls this commit-before-route and it is its best idea.
                held.commit(Entry::Tool {
                    id: ToolCallId::new(call.id.clone()),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                    result: None,
                    thought_signature: None,
                })?;
            }
        }

        for call in &calls {
            // Checked per call, not per round: the entry is already committed, so a stop between
            // two tools leaves a result saying it was never run rather than a call with no answer.
            let output = if cancel.is_requested() {
                axum_tools::Output::error("cancelled before this tool ran")
            } else {
                let arguments = call.parsed().unwrap_or(serde_json::Value::Null);
                registry.call(&call.name, &arguments, ops, &cancel)
            };
            let mut held = session.lock().await;
            held.amend(Entry::Tool {
                id: ToolCallId::new(call.id.clone()),
                name: call.name.clone(),
                args: call.arguments.clone(),
                thought_signature: None,
                result: Some(axum_proto::ToolResult {
                    output: output.content,
                    is_error: output.is_error,
                }),
            })?;
        }

        if cancel.is_requested() {
            session.lock().await.set_status(AgentStatus::Idle);
            return Ok(());
        }
    }

    let mut held = session.lock().await;
    held.commit(Entry::Assistant {
        id: MessageId::new("rounds"),
        text: String::new(),
        thinking: String::new(),
        stop_reason: Some(StopReason::Error),
        error: Some(format!(
            "stopped after {MAX_ROUNDS} rounds of tool use without finishing"
        )),
        signatures: axum_proto::Signatures::default(),
    })?;
    held.set_status(AgentStatus::Idle);
    Ok(())
}

/// The calls a finished turn is waiting on, if any.
fn turn_calls(turn: &Turn) -> Vec<axum_core::PendingCall> {
    match turn.step() {
        Step::RunTools(calls) => calls,
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use axum_journal::JournalError;
    use axum_proto::{MessageId, SessionId, Signatures, ToolCallId, ToolResult};

    fn session(name: &str) -> (Session, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("axum-ctx-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
        (session, dir)
    }

    /// One tool-using round, exactly as the turn loop journals it.
    fn tool_round(session: &mut Session) -> Result<(), JournalError> {
        session.commit(Entry::User {
            id: MessageId::new("u1"),
            text: "read the file".into(),
        })?;
        session.commit(Entry::Assistant {
            id: MessageId::new("a2"),
            text: String::new(),
            thinking: "I should read it".into(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            signatures: Signatures {
                text: None,
                thinking: Some("sig-thinking".into()),
            },
        })?;
        session.commit(Entry::Tool {
            id: ToolCallId::new("c1"),
            name: "read".into(),
            args: r#"{"path":"a.rs"}"#.into(),
            result: Some(ToolResult {
                output: "contents".into(),
                is_error: false,
            }),
            thought_signature: Some("sig-call".into()),
        })?;
        Ok(())
    }

    #[test]
    fn the_call_the_model_made_is_replayed_with_its_result() {
        // A tool result with no preceding tool call is not a conversation. Anthropic rejects
        // it outright; an OpenAI-compatible endpoint takes it and leaves the model with no
        // record of what it asked for, which is worse because it looks like it worked.
        let (mut session, dir) = session("callback");
        tool_round(&mut session).expect("journal");

        let context = context_of(&session);
        let assistant = context
            .messages
            .iter()
            .find(|m| m.role == Role::Assistant)
            .expect("an assistant message");
        let call = assistant
            .content
            .iter()
            .find_map(|c| match c {
                Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => Some((id, name, arguments)),
                _ => None,
            })
            .expect("the assistant asked for a tool, so the message must show it");
        assert_eq!(call.0, "c1");
        assert_eq!(call.1, "read");
        assert_eq!(call.2["path"], "a.rs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_call_comes_before_the_result_it_answers() {
        let (mut session, dir) = session("order");
        tool_round(&mut session).expect("journal");

        let context = context_of(&session);
        let asked = context
            .messages
            .iter()
            .position(|m| {
                m.content
                    .iter()
                    .any(|c| matches!(c, Content::ToolCall { .. }))
            })
            .expect("a call");
        let answered = context
            .messages
            .iter()
            .position(|m| {
                m.content
                    .iter()
                    .any(|c| matches!(c, Content::ToolResult { .. }))
            })
            .expect("a result");
        assert!(asked < answered, "{asked} came after {answered}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_signatures_survive_the_journal() {
        // A reasoning model does not send back reasoning you can re-send; it sends a token
        // standing for it. Dropping it makes the next request a 400 on the providers that
        // check, which is the second round trip of every tool-using turn.
        let (mut session, dir) = session("signatures");
        tool_round(&mut session).expect("journal");

        let context = context_of(&session);
        let assistant = context
            .messages
            .iter()
            .find(|m| m.role == Role::Assistant)
            .expect("an assistant message");
        let thinking = assistant
            .content
            .iter()
            .find_map(|c| match c {
                Content::Thinking { signature, .. } => Some(signature.clone()),
                _ => None,
            })
            .expect("a thinking block");
        assert_eq!(thinking.as_deref(), Some("sig-thinking"));

        let carried = assistant.content.iter().find_map(|c| match c {
            Content::ToolCall {
                thought_signature, ..
            } => Some(thought_signature.clone()),
            _ => None,
        });
        assert_eq!(carried.flatten().as_deref(), Some("sig-call"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_that_only_asked_for_a_tool_is_still_a_message() {
        // Empty text and empty thinking, which is the common shape: the model says nothing and
        // calls something. Dropping it takes the tool call with it.
        let (mut session, dir) = session("silent");
        session
            .commit(Entry::Assistant {
                id: MessageId::new("a1"),
                text: String::new(),
                thinking: String::new(),
                stop_reason: Some(StopReason::ToolUse),
                error: None,
                signatures: Signatures::default(),
            })
            .expect("journal");
        session
            .commit(Entry::Tool {
                id: ToolCallId::new("c1"),
                name: "read".into(),
                args: "{}".into(),
                result: Some(ToolResult {
                    output: "x".into(),
                    is_error: false,
                }),
                thought_signature: None,
            })
            .expect("journal");

        let context = context_of(&session);
        assert!(
            context.messages.iter().any(|m| m
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall { .. }))),
            "{:?}",
            context.messages
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_assistant_message_with_nothing_at_all_is_still_dropped() {
        // The empty entry the turn loop commits before the first delta. Sending it would be
        // a message with no content, which providers reject.
        let (mut session, dir) = session("empty");
        session
            .commit(Entry::Assistant {
                id: MessageId::new("a1"),
                text: String::new(),
                thinking: String::new(),
                stop_reason: None,
                error: None,
                signatures: Signatures::default(),
            })
            .expect("journal");
        assert!(context_of(&session).messages.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
