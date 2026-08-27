//! Running one turn against a provider.
//!
//! The daemon's half of the loop: it owns the socket, the journal and the clock, and holds no
//! agent logic — [`axum_core::Turn`] decides what happens and this drives it.

use crate::session::Session;
use axum_core::{Step, Turn};
use axum_model::StopReason;
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
        (crate::context::of(&held), held.entries().len())
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
    let mut context = crate::context::of(&*session.lock().await);
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
            usage: axum_proto::Usage::default(),
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
            usage: axum_proto::Usage::default(),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(Round {
            turn,
            failed: Some(class),
        });
    }

    let mut held = session.lock().await;

    held.amend(assistant(&id, &turn))?;
    // Idle only when the turn is actually over. A round that stopped for tools is followed by
    // the tools running and another round; saying "idle" in between is a flicker that reads as
    // the end to anything watching the status rather than the transcript.
    if !matches!(turn.state(), axum_core::TurnState::ToolsPending) {
        held.set_status(AgentStatus::Idle);
    }
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
        usage: turn.usage(),
        stop_reason: match turn.state() {
            axum_core::TurnState::Finished(reason) => Some(reason),
            // A message that asked for tools is finished as a message: the model said its
            // piece and stopped. Reporting `None` marked it as still streaming, so no
            // `AssistantEnded` was ever published for it -- and anything waiting for a turn to
            // end had only the idle flicker between rounds to go on, which is not the end.
            axum_core::TurnState::ToolsPending => Some(StopReason::ToolUse),
            _ => None,
        },
        error: None,
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
        crate::compact::needed(&crate::context::of(&held), &backend.model)
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
        usage: axum_proto::Usage::default(),
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
