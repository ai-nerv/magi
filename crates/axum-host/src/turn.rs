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
    /// The family's client libraries, so a Lua tool can talk to a sibling.
    pub clients: Vec<(String, String)>,
    /// Where the session is rooted, which is what tools resolve paths against.
    pub cwd: std::path::PathBuf,
    /// Permissions a configuration granted before anybody was asked anything.
    ///
    /// A rule written down is a question already answered, so these go into the ledger at
    /// startup rather than being prompted for.
    pub grants: Vec<axum_proto::permit::Grant>,
    /// Environment every process this session starts is given, beside the mandatory pairs.
    pub environ: std::collections::BTreeMap<String, String>,
    /// Whether the file tools refuse paths outside `cwd`.
    ///
    /// Off unless a config asks. See [`axum_tools::ops::Real`] for why a wall only the careful
    /// tools obey is worse than no wall.
    pub confine: bool,
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
    /// What the model is told it is, before the conversation starts.
    ///
    /// Assembled once, when the daemon starts. Rebuilding it per turn would let a project file
    /// change what the model was told between one message and the next, with nothing in the
    /// transcript to say so.
    pub system: Option<String>,
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
        (crate::context::of(&held), held.entries().to_vec())
    };
    let Some(covered) = crate::compact::covers(&entries) else {
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
            &axum_provider::client::Call {
                adapter,
                provider: &backend.provider,
                model: &backend.model,
                context: &asked,
                options: &backend.options,
            },
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
    context.system.clone_from(&backend.system);

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

    // One channel for both, because the order matters: a delta from the second attempt arriving
    // before the retry that discarded the first would be thrown away with it. Two channels
    // cannot promise that; one can.
    //
    // A channel at all because the callbacks are synchronous and the session is behind an async
    // lock — and because the whole value of saying "retrying" is saying it *during* the wait. A
    // person watching a spinner for forty seconds needs to know it is a wait and not a hang.
    let (arrivals, mut arriving) = tokio::sync::mpsc::unbounded_channel();
    let retries = arrivals.clone();
    let outcome = {
        // The provider call is raced against the interrupt rather than polled after it: a model
        // mid-answer holds this future for as long as it keeps talking, and a flag checked when
        // it returns is a stop that arrives once the work it was stopping is already paid for.
        let call = axum_provider::client::Call {
            adapter,
            provider: &backend.provider,
            model: &backend.model,
            context: &context,
            options: &backend.options,
        };
        let streaming = client.stream_reporting(
            &call,
            // A closed receiver means the turn is over; there is nobody to tell.
            |delta| {
                let _ = arrivals.send(Arrival::Delta(delta));
            },
            |retry| {
                let _ = retries.send(Arrival::Retrying(retry));
            },
        );
        let mut streaming = std::pin::pin!(streaming);
        loop {
            tokio::select! {
                biased;
                () = cancel.requested() => break Ok(()),
                Some(arrival) = arriving.recv() => {
                    match arrival {
                        // Applied and published as it arrives, which is the whole milestone.
                        // Revised rather than amended: an amendment writes the message to disk
                        // and flushes, which per token would write it once per token, each copy
                        // longer than the last.
                        Arrival::Delta(delta) => {
                            turn.apply(delta);
                            session.lock().await.revise(assistant(&id, &turn));
                        }
                        Arrival::Retrying(retry) => {
                            // What the attempt published has to be taken back. The transcript
                            // can say so — a message that is not an extension of itself is
                            // described in full rather than as an append — so this is one
                            // revision back to nothing.
                            turn = Turn::new();
                            let mut held = session.lock().await;
                            held.revise(assistant(&id, &turn));
                            // The UI has had a display for this since M0 that nothing ever set:
                            // during an overload a person saw "Thinking" for a minute with no
                            // sign that anything had gone wrong or would be tried again.
                            held.set_status(AgentStatus::Retrying {
                                attempt: retry.attempt,
                                max_attempts: retry.max_attempts,
                                delay_ms: u64::try_from(retry.delay.as_millis())
                                    .unwrap_or(u64::MAX),
                            });
                        }
                    }
                }
                outcome = &mut streaming => break outcome,
            }
        }
    };

    // Whatever the select! did not get to before the stream ended. A delta and the end of the
    // stream can arrive in the same poll, and the loop breaks on the outcome.
    while let Ok(arrival) = arriving.try_recv() {
        if let Arrival::Delta(delta) = arrival {
            turn.apply(delta);
        }
    }
    session.lock().await.revise(assistant(&id, &turn));

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

/// Something the provider call said, in the order it said it.
///
/// One type down one channel, because the order is what makes a retraction safe. A delta from
/// the second attempt arriving before the retry that discarded the first would be discarded
/// with it, and two channels have no way to promise it does not.
enum Arrival {
    /// Part of the answer.
    Delta(axum_provider::api::Delta),
    /// The attempt failed and another is starting. Everything published so far is retracted.
    Retrying(axum_provider::client::Retrying),
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

        // Where each call was journalled, so its answer lands on its own entry. Amending "the
        // last entry" is right for a message still streaming and wrong here: a round commits
        // every call before running any, so by the time the first result arrives there are two
        // more entries after it. Every result but the last went to the wrong entry and was then
        // overwritten, leaving calls with `result: null` that the model had made and never got
        // an answer to.
        let mut at = Vec::with_capacity(calls.len());
        {
            let mut held = session.lock().await;
            held.set_status(AgentStatus::Working {
                label: "Running tools".into(),
            });
            for call in &calls {
                // Journalled before it is run, and before the registry is consulted: a call
                // that went nowhere is still something the transcript can account for. Tau
                // calls this commit-before-route and it is its best idea.
                at.push(held.commit(Entry::Tool {
                    id: ToolCallId::new(call.id.clone()),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                    result: None,
                    thought_signature: None,
                })?);
            }
        }

        // Sequential preparation, parallel execution, results in source order — Pi's shape
        // (`agent-loop.ts:489-554`), and the only one available here. `Tool` is deliberately not
        // `Send`, because a Lua tool runs in a VM that is not, so this cannot be threads. It does
        // not need to be: a peer is another *process*, so writing its request and coming back for
        // the answer is all the concurrency there is to have. Three calls to three peers cost the
        // slowest rather than the sum; a built-in or a Lua tool has nothing to overlap and says
        // so, and runs where it stands.
        //
        // Preparation stays one at a time on purpose. Checking arguments is cheap, but asking a
        // person for permission is not, and two prompts racing onto one screen is not a faster
        // round but an unanswerable one.
        let mut prepared = Vec::with_capacity(calls.len());
        for call in &calls {
            if cancel.is_requested() {
                prepared.push(None);
                continue;
            }
            prepared.push(Some(registry.prepare(&call.name, &call.arguments, ops)));
        }

        for ((call, prepared), at) in calls.iter().zip(prepared).zip(at) {
            // Checked per call, not per round: the entry is already committed, so a stop between
            // two tools leaves a result saying it was never run rather than a call with no answer.
            let output = match prepared {
                // A call already in flight is collected even after an interrupt: the peer is
                // running it either way, and the answer is owed to the entry that was committed
                // before it went out.
                Some(prepared) if !cancel.is_requested() || prepared.in_flight() => {
                    registry.finish(prepared, ops, &cancel)
                }
                _ => axum_tools::Output::error("cancelled before this tool ran"),
            };
            let mut held = session.lock().await;
            held.amend_at(
                at,
                Entry::Tool {
                    id: ToolCallId::new(call.id.clone()),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                    thought_signature: None,
                    result: Some(axum_proto::ToolResult {
                        output: output.content,
                        is_error: output.is_error,
                    }),
                },
            )?;
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
