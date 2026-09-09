//! Running one turn against a provider: the daemon owns the socket, the transcript and the clock,
//! and holds no agent logic — [`magi_core::Turn`] decides what happens and this drives it.

use crate::session::Session;
use magi_core::{Step, Turn};
use magi_model::StopReason;
use magi_proto::{AgentStatus, Entry, MessageId, ToolCallId};
use magi_tools::{Ops, Registry};

/// What the daemon needs to reach a model. Plain data, and sendable: the protocol it names is built
/// on the worker's own thread, because a Lua VM is neither `Send` nor `Sync`.
#[derive(Debug, Clone)]
pub struct Backend {
    pub tools: Vec<(String, String)>,
    pub clients: Vec<(String, String)>,
    /// The SHA-256 casper's program must hash to, if this configuration pinned one.
    pub casper: Option<String>,
    /// What this session tells casper to be, on every spawn, since casper is one process per call.
    pub casper_configure: String,
    pub cwd: std::path::PathBuf,
    /// Permissions a configuration granted in advance; they go into the ledger at startup.
    pub grants: Vec<magi_proto::permit::Grant>,
    pub environ: std::collections::BTreeMap<String, String>,
    /// Whether the file tools refuse paths outside `cwd`. See [`magi_tools::ops::Real`].
    pub confine: bool,
    /// Which model to ask for, as melchior names it: `provider/model`. A name and nothing else.
    pub model: String,
    /// The program that owns the model, found on `PATH`. Named per backend, not compiled in.
    pub mind: String,
    pub wants: magi_proto::ask::Wants,
    /// How much this model will read, as melchior's card reported it. Carried rather than looked up.
    pub context_window: Option<u64>,
    /// What the model is told it is. Assembled once, when the daemon starts.
    pub system: Option<String>,
}

/// Run one turn and journal what it produced. The entry is written before the turn ends, so a UI
/// attaching mid-turn extends a partial message and a crash leaves one rather than nothing.
async fn one_turn(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    tools: Vec<magi_model::Tool>,
    cancel: &crate::cancel::Cancel,
    remembered: Option<&magi_model::Message>,
) -> Result<Round, crate::HostError> {
    let mut context = crate::context::of(&*session.lock().await);
    context.tools = tools;
    context.system.clone_from(&backend.system);
    if let Some(remembered) = remembered {
        crate::injecting::put(&mut context, remembered.clone());
    }

    {
        let mut held = session.lock().await;
        held.set_status(AgentStatus::Working {
            label: "Thinking".into(),
        });
    }

    let id = MessageId::new(format!("a{}", session.lock().await.cursor().next().0));
    let mut turn = Turn::new();
    let mut retries_seen: Vec<(u32, u32, u64)> = Vec::new();

    session.lock().await.commit(assistant(&id, &turn))?;

    // One channel for both: a delta from the second attempt arriving before the retry that
    // discarded the first would be thrown away with it.
    let (arrivals, mut arriving) = tokio::sync::mpsc::unbounded_channel();
    let retries = arrivals.clone();
    let outcome = {
        // Raced against the interrupt rather than polled after it: a model mid-answer holds this
        // future for as long as it keeps talking, and a flag checked on return stops nothing.
        let streaming = crate::broker::ask_through(
            &backend.mind,
            &backend.model,
            &context,
            &backend.wants,
            |delta| {
                // A closed receiver means the turn is over; there is nobody to tell.
                let _ = arrivals.send(Arrival::Delta(delta));
            },
            |retry| {
                let _ = retries.send(Arrival::Retrying {
                    attempt: retry.attempt,
                    max_attempts: retry.max_attempts,
                    delay_ms: retry.delay_ms,
                });
            },
        );
        let mut streaming = std::pin::pin!(streaming);
        loop {
            tokio::select! {
                biased;
                () = cancel.requested() => break Ok(()),
                Some(arrival) = arriving.recv() => {
                    match arrival {
                        // Revised rather than amended: an amendment writes to disk and flushes.
                        Arrival::Delta(delta) => {
                            turn.apply(delta);
                            session.lock().await.revise(assistant(&id, &turn));
                        }
                        Arrival::Retrying { attempt, max_attempts, delay_ms } => {
                            retries_seen.push((attempt, max_attempts, delay_ms));
                            // What the attempt published has to be taken back, to nothing.
                            turn = Turn::new();
                            let mut held = session.lock().await;
                            held.revise(assistant(&id, &turn));
                            // Nothing ever set this, so an overload showed "Thinking" for a minute.
                            held.set_status(AgentStatus::Retrying {
                                attempt,
                                max_attempts,
                                delay_ms,
                            });
                        }
                    }
                }
                outcome = &mut streaming => break outcome,
            }
        }
    };

    // A delta and the end of the stream can arrive in the same poll, and the loop breaks on the outcome.
    while let Ok(arrival) = arriving.try_recv() {
        match arrival {
            Arrival::Delta(delta) => turn.apply(delta),
            // A retry that landed in the same poll the stream ended in. Handled exactly as the loop
            // handles it: everything after a retry in a FIFO channel belongs to the attempt after it.
            Arrival::Retrying {
                attempt,
                max_attempts,
                delay_ms,
            } => {
                retries_seen.push((attempt, max_attempts, delay_ms));
                turn = Turn::new();
                let mut held = session.lock().await;
                held.revise(assistant(&id, &turn));
                held.set_status(AgentStatus::Retrying {
                    attempt,
                    max_attempts,
                    delay_ms,
                });
            }
        }
    }
    session.lock().await.revise(assistant(&id, &turn));

    // Whatever arrived before the interrupt is kept: the model said it.
    if cancel.is_requested() {
        turn.abort(StopReason::Aborted);
        let mut held = session.lock().await;
        held.amend(Entry::Assistant {
            id,
            text: turn.text().to_owned(),
            thinking: turn.thinking().to_owned(),
            stop_reason: Some(StopReason::Aborted),
            error: None,
            signatures: magi_proto::Signatures {
                text: None,
                thinking: turn.signature().map(str::to_owned),
            },
            usage: magi_proto::Usage::default(),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(Round {
            turn,
            failed: None,
            retries: retries_seen,
        });
    }

    if let Err(error) = outcome {
        // An error is a value, not an exception: the transcript stays well-formed.
        let refused = error.why;
        turn.abort(StopReason::Error);
        let mut held = session.lock().await;
        held.amend(Entry::Assistant {
            id,
            text: turn.text().to_owned(),
            thinking: turn.thinking().to_owned(),
            stop_reason: Some(StopReason::Error),
            error: Some(error.message),
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(Round {
            turn,
            failed: Some(refused),
            retries: retries_seen,
        });
    }

    let mut held = session.lock().await;

    held.amend(assistant(&id, &turn))?;
    // Idle only when the turn is over; a round that stopped for tools is followed by another round.
    if !matches!(turn.state(), magi_core::TurnState::ToolsPending) {
        held.set_status(AgentStatus::Idle);
    }
    Ok(Round {
        turn,
        failed: None,
        retries: retries_seen,
    })
}

/// What one round produced. The turn on its own cannot say why it stopped, and by the time a
/// failure is an error entry the class is gone — so an overflow can be answered by compacting.
struct Round {
    turn: Turn,
    /// Set when the provider refused, and the class it refused with.
    failed: Option<magi_proto::ask::Refusal>,
    /// Every retry this round took, as `(attempt, of, delay_ms)`; the loop that owns the watchers
    /// is what reports them.
    retries: Vec<(u32, u32, u64)>,
}

/// Something the provider call said, in the order it said it. One type down one channel, because
/// the order is what makes a retraction safe.
enum Arrival {
    Delta(magi_model::Delta),
    /// The attempt failed and another is starting. Everything published so far is retracted.
    Retrying {
        /// Which attempt just failed, counting from one.
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
    },
}

/// The assistant entry for a turn in its current state.
fn assistant(id: &MessageId, turn: &Turn) -> Entry {
    Entry::Assistant {
        id: id.clone(),
        text: turn.text().to_owned(),
        thinking: turn.thinking().to_owned(),
        // Carried into the journal, because the journal is what the next request is built from.
        signatures: magi_proto::Signatures {
            text: None,
            thinking: turn.signature().map(str::to_owned),
        },
        usage: turn.usage(),
        stop_reason: match turn.state() {
            magi_core::TurnState::Finished(reason) => Some(reason),
            // A message that asked for tools is finished as a message; `None` marked it streaming.
            magi_core::TurnState::ToolsPending => Some(StopReason::ToolUse),
            _ => None,
        },
        error: None,
    }
}

/// Tell the watchers about permissions decided since the last time: two events per question,
/// adjacent and in order. See [`magi_tools::watching::Pending`] for why not where they are decided.
async fn permissions(
    registry: &Registry,
    ops: &dyn Ops,
    scribe: &crate::scribe::Held,
    cursor: magi_proto::Cursor,
) {
    for noted in ops.noticed() {
        registry.saw(&magi_tools::Event::Asked {
            verb: &noted.verb,
            about: &noted.about,
        });
        registry.saw(&magi_tools::Event::Answered {
            verb: &noted.verb,
            about: &noted.about,
            allowed: noted.allowed,
        });
        // And to the memory layer: a permission is not a transcript entry, so before this it
        // reached only this session's VM. Best effort, never awaited on the path of a refusal.
        let said = format!(
            "{} {} was {}",
            noted.verb,
            noted.about,
            if noted.allowed { "allowed" } else { "refused" }
        );
        let mut open = scribe.lock().await;
        if let Some(scribe) = open.as_mut()
            && let Err(why) = scribe.noticed(cursor, "permission", &said).await
        {
            magi_model::noted!("turn: the trace could not be recorded: {why}");
        }
    }
}

/// Rounds of tool use one prompt may take before the loop gives up.
const MAX_ROUNDS: usize = 24;

/// Run a prompt to completion: provider, tools, provider, until the turn ends. Every result is
/// journalled as its own entry, so the transcript shows what was asked and what came back.
pub async fn run(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    registry: &Registry,
    ops: &dyn Ops,
    scribe: &crate::scribe::Held,
) -> Result<(), crate::HostError> {
    // Taken once: the handle is a clone of shared state, so a mid-round stop is visible through it.
    let cancel = session.lock().await.cancel();

    // Whether to compact is balthasar's answer, not a threshold here. Before the first round, not
    // before every one: compacting between rounds summarises a conversation still in progress.
    compact(session, backend, registry, scribe, PATIENCE).await;

    // Once per prompt, and after any compaction: the recall is about what the person asked, and
    // recalling first would spend the budget on a window that is about to change shape.
    let (remembered, injection) = remembered(session, backend, scribe).await;

    // One reactive compaction per prompt: a second overflow means the kept tail alone will not fit.
    let mut compacted = false;

    for _ in 0..MAX_ROUNDS {
        // A turn is one exchange with the model, and this is where a watcher learns of it.
        registry.saw(&magi_tools::Event::TurnBegan {
            model: &backend.model,
        });
        let began = std::time::Instant::now();

        let round = one_turn(
            session,
            backend,
            registry.declarations(),
            &cancel,
            remembered.as_ref(),
        )
        .await?;

        for (attempt, of, delay_ms) in &round.retries {
            registry.saw(&magi_tools::Event::Retried {
                mind: &backend.mind,
                attempt: *attempt,
                of: *of,
                delay_ms: *delay_ms,
            });
        }
        registry.saw(&magi_tools::Event::TurnEnded {
            model: &backend.model,
            took_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
            ok: round.failed.is_none(),
        });

        // The estimate above is rough; this is the provider's own answer. The failed round stays.
        if round.failed == Some(magi_proto::ask::Refusal::Overflow) && !compacted {
            compacted = true;
            if compact(session, backend, registry, scribe, INSISTENCE).await {
                continue;
            }
        }
        let turn = round.turn;

        // An interrupted turn is already journalled as aborted; continuing would abort the next too.
        if cancel.is_requested() {
            return Ok(());
        }

        // `length` can land mid-arguments, and truncated JSON can still parse as schema-valid.
        let poisoned = turn.poisoned_results();
        if !poisoned.is_empty() {
            let mut held = session.lock().await;
            for (call, _) in turn_calls(&turn).into_iter().zip(poisoned) {
                held.commit(Entry::Tool {
                    id: ToolCallId::new(call.id.clone()),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                    // No description emits one yet; the journal must hold what the model layer does.
                    thought_signature: None,
                    result: Some(magi_proto::ToolResult {
                        output: "The response was truncated before this call was complete. \
                                 Re-issue it with complete arguments."
                            .to_owned(),
                        is_error: true,
                        shown: None,
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

        // Where each call was journalled, so its answer lands on its own entry. A round commits
        // every call before running any, so amending "the last entry" puts every result but one wrong.
        let mut at = Vec::with_capacity(calls.len());
        {
            let mut held = session.lock().await;
            held.set_status(AgentStatus::Working {
                label: "Running tools".into(),
            });
            for call in &calls {
                // Journalled before it is run and before the registry is consulted, so a call that
                // went nowhere is still something the transcript can account for.
                at.push(held.commit(Entry::Tool {
                    id: ToolCallId::new(call.id.clone()),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                    result: None,
                    thought_signature: None,
                })?);
            }
        }

        // Sequential preparation, parallel execution, results in source order. `Tool` is
        // deliberately not `Send`, so this cannot be threads; a peer is another process, so writing
        // its request and coming back is all the concurrency there is. Preparation stays one at a
        // time: two permission prompts racing onto one screen is an unanswerable round.
        let mut prepared = Vec::with_capacity(calls.len());
        for call in &calls {
            if cancel.is_requested() {
                prepared.push(None);
                continue;
            }
            prepared.push(Some(registry.prepare(&call.name, &call.arguments, ops)));
        }
        // Preparation is where permission is asked; told here because the watchers live on this thread.
        permissions(registry, ops, scribe, session.lock().await.cursor()).await;

        for ((call, prepared), at) in calls.iter().zip(prepared).zip(at) {
            // Checked per call: the entry is committed, so a stop leaves a result, not a bare call.
            let output = match prepared {
                // A call in flight is collected even after an interrupt: the peer runs it either way.
                Some(prepared) if !cancel.is_requested() || prepared.in_flight() => {
                    registry.finish(prepared, ops, &cancel)
                }
                _ => magi_tools::Output::error("cancelled before this tool ran"),
            };
            // The other half of the loop: what the turn did with what it was given, the only signal
            // balthasar has. After the tool, before the entry is amended, and off with no ledger.
            if let Some(injection) = &injection {
                acted_on(scribe, injection, call, output.is_error).await;
            }
            let mut held = session.lock().await;
            held.amend_at(
                at,
                Entry::Tool {
                    id: ToolCallId::new(call.id.clone()),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                    thought_signature: None,
                    result: Some(magi_proto::ToolResult {
                        output: output.content,
                        is_error: output.is_error,
                        // The other face, carried into the transcript so the renderer can draw what
                        // the tool meant rather than guess at it from the text.
                        shown: output.shown,
                    }),
                },
            )?;
        }
        // Again after the tools have run: a Lua tool that shells out asks its own questions.
        permissions(registry, ops, scribe, session.lock().await.cursor()).await;

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
        signatures: magi_proto::Signatures::default(),
        usage: magi_proto::Usage::default(),
    })?;
    held.set_status(AgentStatus::Idle);
    Ok(())
}

/// The calls a finished turn is waiting on, if any.
fn turn_calls(turn: &Turn) -> Vec<magi_core::PendingCall> {
    match turn.step() {
        Step::RunTools(calls) => calls,
        _ => Vec::new(),
    }
}

/// Compaction, and what balthasar is asked and told.
#[path = "turn/memory.rs"]
mod memory;
use memory::{INSISTENCE, PATIENCE, acted_on, compact, remembered};
