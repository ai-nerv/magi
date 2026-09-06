//! Running one turn against a provider.
//!
//! The daemon's half of the loop: it owns the socket, the journal and the clock, and holds no
//! agent logic — [`magi_core::Turn`] decides what happens and this drives it.

use crate::session::Session;
use magi_core::{Step, Turn};
use magi_model::StopReason;
use magi_proto::{AgentStatus, Entry, MessageId, ToolCallId};
use magi_tools::{Ops, Registry};

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
    /// The SHA-256 casper's program must hash to, if this configuration pinned one.
    pub casper: Option<String>,
    /// Where the session is rooted, which is what tools resolve paths against.
    pub cwd: std::path::PathBuf,
    /// Permissions a configuration granted before anybody was asked anything.
    ///
    /// A rule written down is a question already answered, so these go into the ledger at
    /// startup rather than being prompted for.
    pub grants: Vec<magi_proto::permit::Grant>,
    /// Environment every process this session starts is given, beside the mandatory pairs.
    pub environ: std::collections::BTreeMap<String, String>,
    /// Whether the file tools refuse paths outside `cwd`.
    ///
    /// Off unless a config asks. See [`magi_tools::ops::Real`] for why a wall only the careful
    /// tools obey is worse than no wall.
    pub confine: bool,
    /// Which model to ask for, as melchior names it: `provider/model`.
    ///
    /// A name and nothing else. magi does not know which protocol this model speaks, where it
    /// lives, or what credential it takes -- melchior owns all three, and a harness that held a
    /// second opinion about any of them would be a second thing to keep in step.
    pub model: String,
    /// The program that owns the model, found on `PATH`.
    ///
    /// [`crate::broker::MELCHIOR`] in every session. Named per backend rather than compiled in
    /// so a test can point one turn at a stand-in: `PATH` is process-wide, and tests that fought
    /// over it would be tests that pass alone and fail together.
    pub mind: String,
    /// What to ask for beyond the conversation.
    pub wants: magi_proto::ask::Wants,
    /// How much this model will read, as melchior's card reported it.
    ///
    /// Carried rather than looked up, because the only thing magi does with it is decide when to
    /// compact -- and asking melchior that on every turn would be a process per decision.
    pub context_window: Option<u64>,
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
async fn compact(session: &tokio::sync::Mutex<Session>, backend: &Backend) -> bool {
    let entries = {
        let held = session.lock().await;
        held.entries().to_vec()
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

    // **The messages of exactly the entries being replaced.** These were two boundaries once:
    // the journal recorded `entries.len() - KEEP` and the summariser was given
    // `messages.len() - KEEP`, computed independently in two spaces that agree only when every
    // entry makes exactly one message. A `Notice`, a `Branch`, a `Compaction` and an assistant
    // entry that errored each make none, so every one of them in the head of the transcript
    // pushed the entry cut past the message cut — and what fell between was declared summarised
    // without ever being shown to the summariser.
    let asked = crate::compact::request(&crate::context::of_entries(&entries[..covered]));
    let mut turn = magi_core::Turn::new();
    let mut deltas = Vec::new();
    // The same mind that answers a turn writes the summary of one. Collected rather than
    // streamed: nobody watches a compaction, and the entry is written once at the end.
    let outcome = crate::broker::ask_through(
        &backend.mind,
        &backend.model,
        &asked,
        &backend.wants,
        |delta| deltas.push(delta),
        |_| {},
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

/// How long a recall may hold up a turn.
///
/// Generous for a local socket and short enough that nobody notices it. The point is not to bound
/// balthasar — it bounds itself — but to make the turn independent of whether it does.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(250);

/// Ask balthasar what it would have sent, and say how it differs from what magi will.
///
/// Nothing acts on the answer. It exists so the difference is measurable at all: magi compacts
/// with `KEEP` and a character estimate, balthasar decides per memory with everything it knows
/// about the run, and until now there was no way to see that they disagree — let alone by how
/// much.
///
/// Best effort, on the same clock as everything else here. A balthasar that has observed nothing
/// refuses this, which is the ordinary answer for a harness that has not streamed its turns.
async fn second_opinion(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    scribe: &crate::scribe::Held,
) {
    let Some(window) = backend.context_window else {
        return;
    };
    let ours = {
        let held = session.lock().await;
        crate::compact::covers(held.entries()).unwrap_or(0)
    };
    let theirs = tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        open.as_mut()?.would_send(window).await.ok()
    })
    .await
    .ok()
    .flatten();
    if let Some(theirs) = theirs {
        let counted = |what: &str| {
            theirs
                .get(what)
                .and_then(|v| v.as_array())
                .map_or(0, Vec::len)
        };
        magi_model::noted!(
            "compact: magi replaces {ours} entries; balthasar would keep {}, mask {}, \
             drop {} and summarise {} — {}",
            counted("keep"),
            counted("mask"),
            counted("drop"),
            counted("summarise"),
            theirs
                .get("why")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no reason given")
        );
    }
}

/// What this project remembers about the prompt in front of it, as a message.
///
/// The half of the memory layer that was never connected. The transcript has always flowed *to*
/// balthasar through [`crate::scribe`], and it comes back three ways — a surface may ask, a model
/// may call `recall` as a tool, and `magi doctor` will say the layer is there. All three need
/// somebody to ask first, which a model that has forgotten something cannot do.
///
/// Keyed on the last thing the person said, because that is what the turn is about. Best effort
/// throughout: a balthasar that is missing, wedged or refusing costs the turn nothing.
async fn remembered(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    scribe: &crate::scribe::Held,
) -> (Option<magi_model::Message>, Option<String>) {
    let Some(window) = backend.context_window else {
        return (None, None);
    };
    let query = {
        let held = session.lock().await;
        match crate::context::last_asked(&held) {
            Some(query) => query,
            None => return (None, None),
        }
    };

    // **On a clock, because this is in front of the person's turn.** A memory layer that is
    // slow, wedged, or busy compacting its own store must cost the conversation nothing — that
    // is what makes recalling unconditional rather than a setting somebody has to find. A local
    // socket answers this in single-digit milliseconds; anything that does not is not going to
    // be worth waiting for.
    let asked = std::time::Instant::now();
    let found = tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        open.as_mut()?
            .nearest(&query, crate::injecting::MOST)
            .await
            .inspect_err(|why| magi_model::noted!("turn: recall was refused: {why}"))
            .ok()
    })
    .await
    .inspect_err(|_| magi_model::noted!("turn: recall did not answer within {PATIENCE:?}"))
    .ok()
    .flatten();
    let Some(found) = found else {
        return (None, None);
    };
    let window = usize::try_from(window).unwrap_or(usize::MAX);
    let waited = asked.elapsed();
    // The id travels with the message. It is what makes an outcome attributable later: balthasar
    // decides for itself whether an action followed any of the memories it gave, and it can only
    // do that against the injection it served them under.
    let message = crate::injecting::preface(&found.memories, window);
    // The price of asking, every turn, in the two units somebody would judge it by. balthasar
    // measures whether memory earns its place and can only see its own side; this is the half
    // the harness pays and the half nothing recorded.
    if let Some(message) = &message {
        let cost = crate::injecting::Cost::of(message);
        magi_model::noted!(
            "memory: {} asserted and {} hedged, {} tokens, recalled in {}ms",
            cost.asserted,
            cost.hedged,
            cost.tokens,
            waited.as_millis()
        );
    }
    (message, found.injection)
}

/// Report one finished tool against the injection that preceded it.
///
/// The action is one string — a command, a path, a query — because that is what balthasar hashes
/// and keeps a digest of. The arguments themselves do not leave this process.
///
/// `recall` and `remember` are skipped: a call *to* the memory layer is not an action taken on
/// what it said, and counting it would have every injection look used.
async fn acted_on(
    scribe: &crate::scribe::Held,
    injection: &str,
    call: &magi_core::PendingCall,
    failed: bool,
) {
    if matches!(call.name.as_str(), "recall" | "remember" | "forget" | "why") {
        return;
    }
    let action = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .ok()
        .and_then(|args| {
            ["command", "path", "query", "pattern"]
                .iter()
                .find_map(|name| {
                    args.get(*name)
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
        })
        .unwrap_or_default();

    // On the same clock as the recall, and for the same reason: this is instrumentation, and a
    // memory layer having a bad minute must not be something the conversation waits for.
    let reported = tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        if let Some(open) = open.as_mut() {
            let _ = open
                .acted(injection, &call.name, &action, !failed)
                .await
                .inspect_err(|why| magi_model::noted!("turn: an outcome was refused: {why}"));
        }
    })
    .await;
    if reported.is_err() {
        magi_model::noted!("turn: an outcome did not land within {PATIENCE:?}");
    }
}

/// Run one turn and journal what it produced.
///
/// Deltas are published as they arrive and the entry is amended as it grows, so a UI attaching
/// mid-turn sees the same partial message a UI that was there all along sees. A crash leaves
/// the partial message rather than nothing, which is why the entry is written before it ends.
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
        // melchior owns the model. magi gathers the context and writes down the answer; which
        // protocol this model speaks, where it lives and what credential it takes are not
        // magi's to know, and there is no second opinion here to drift from melchior's.
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
                        // Applied and published as it arrives, which is the whole milestone.
                        // Revised rather than amended: an amendment writes the message to disk
                        // and flushes, which per token would write it once per token, each copy
                        // longer than the last.
                        Arrival::Delta(delta) => {
                            turn.apply(delta);
                            session.lock().await.revise(assistant(&id, &turn));
                        }
                        Arrival::Retrying { attempt, max_attempts, delay_ms } => {
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

    // Whatever the select! did not get to before the stream ended. A delta and the end of the
    // stream can arrive in the same poll, and the loop breaks on the outcome.
    while let Ok(arrival) = arriving.try_recv() {
        match arrival {
            Arrival::Delta(delta) => turn.apply(delta),
            // **A retry that landed in the same poll the stream ended in.** This arm used to be
            // absent, so the arrival was read out of the channel and dropped: the status was
            // never set and nothing was ever published. It cost a person the one thing they
            // needed to know — that the answer took two attempts and the first was thrown away —
            // and it did so about one turn in three, which is how it was found.
            //
            // Handled exactly as the loop handles it, because the ordering is the same: a retry
            // is announced before the attempt that follows it streams, and everything after it
            // in a FIFO channel is that attempt. Resetting here discards the abandoned attempt's
            // text and keeps the one that succeeded.
            Arrival::Retrying {
                attempt,
                max_attempts,
                delay_ms,
            } => {
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
            signatures: magi_proto::Signatures {
                text: None,
                thinking: turn.signature().map(str::to_owned),
            },
            usage: magi_proto::Usage::default(),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(Round { turn, failed: None });
    }

    if let Err(error) = outcome {
        // An error is a value, not an exception: the transcript stays well-formed and the UI
        // needs no error branch. Pi's discipline, and the reason its renderer has none.
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
        });
    }

    let mut held = session.lock().await;

    held.amend(assistant(&id, &turn))?;
    // Idle only when the turn is actually over. A round that stopped for tools is followed by
    // the tools running and another round; saying "idle" in between is a flicker that reads as
    // the end to anything watching the status rather than the transcript.
    if !matches!(turn.state(), magi_core::TurnState::ToolsPending) {
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
    failed: Option<magi_proto::ask::Refusal>,
}

/// Something the provider call said, in the order it said it.
///
/// One type down one channel, because the order is what makes a retraction safe. A delta from
/// the second attempt arriving before the retry that discarded the first would be discarded
/// with it, and two channels have no way to promise it does not.
enum Arrival {
    /// Part of the answer.
    Delta(magi_model::Delta),
    /// The attempt failed and another is starting. Everything published so far is retracted.
    /// The mind said an attempt failed and another is starting.
    Retrying {
        /// Which attempt just failed, counting from one.
        attempt: u32,
        /// How many will be made in all.
        max_attempts: u32,
        /// How long before the next one.
        delay_ms: u64,
    },
}

/// The assistant entry for a turn in its current state.
fn assistant(id: &MessageId, turn: &Turn) -> Entry {
    Entry::Assistant {
        id: id.clone(),
        text: turn.text().to_owned(),
        thinking: turn.thinking().to_owned(),
        // Carried into the journal, because the journal is what the next request is built
        // from. Captured by the turn and then dropped here was the whole of the bug.
        signatures: magi_proto::Signatures {
            text: None,
            thinking: turn.signature().map(str::to_owned),
        },
        usage: turn.usage(),
        stop_reason: match turn.state() {
            magi_core::TurnState::Finished(reason) => Some(reason),
            // A message that asked for tools is finished as a message: the model said its
            // piece and stopped. Reporting `None` marked it as still streaming, so no
            // `AssistantEnded` was ever published for it -- and anything waiting for a turn to
            // end had only the idle flicker between rounds to go on, which is not the end.
            magi_core::TurnState::ToolsPending => Some(StopReason::ToolUse),
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
    registry: &Registry,
    ops: &dyn Ops,
    scribe: &crate::scribe::Held,
) -> Result<(), crate::HostError> {
    // Taken once: the handle is a clone of shared state, so a stop asked for mid-round is
    // visible through it without going back to the session for a fresh one.
    let cancel = session.lock().await.cancel();

    // Before the first round, not before every one: a turn adds at most a few messages, and
    // compacting between rounds of one prompt would summarise a conversation the model is
    // still in the middle of.
    let over = {
        let held = session.lock().await;
        crate::compact::needed(&crate::context::of(&held), backend.context_window)
    };
    if over {
        // **What balthasar would have sent, beside what magi did.** Structured eviction over
        // blind truncation is the thing a memory layer is for, and balthasar has the apparatus;
        // what a model is shown is still the harness's to decide, and a compaction that depended
        // on another process would change shape when that process was upgraded. Recorded so the
        // two can be compared — obeying it is a decision to take once there is a number.
        second_opinion(session, backend, scribe).await;
        compact(session, backend).await;
    }

    // **Once per prompt, and after any compaction.** A tool-using turn goes round several times
    // and the recall is about what the person asked, not about what the model has just read; and
    // recalling before a compaction would spend the budget on a window that is about to change
    // shape. Nothing when there is no balthasar, which is the session magi had before there was
    // one.
    let (remembered, injection) = remembered(session, backend, scribe).await;

    // One reactive compaction per prompt. A second overflow after summarising is not a
    // conversation that is too long -- it is one whose kept tail alone will not fit, and
    // compacting again would summarise the summary and still fail.
    let mut compacted = false;

    for _ in 0..MAX_ROUNDS {
        let round = one_turn(
            session,
            backend,
            registry.declarations(),
            &cancel,
            remembered.as_ref(),
        )
        .await?;

        // The estimate above is deliberately rough; this is the provider's own answer. The
        // failed round stays in the transcript, because a reader who notices the model
        // forgetting something deserves to see that this is why.
        if round.failed == Some(magi_proto::ask::Refusal::Overflow) && !compacted {
            compacted = true;
            if compact(session, backend).await {
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
                _ => magi_tools::Output::error("cancelled before this tool ran"),
            };
            // **The other half of the loop.** Memories were put in front of this turn; this says
            // what the turn then did, which is the only signal balthasar has for whether any of
            // them were worth offering. Without it a memory layer ranks by recency and
            // similarity forever and never by whether anything it gave was used.
            //
            // After the tool, before the entry is amended: the answer is known and the lock is
            // not held. Best effort, and off entirely when balthasar keeps no ledger.
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
                        // The other face, carried into the transcript so the renderer can draw
                        // what the tool *meant* rather than guess at it from the text. A tool
                        // that said nothing about how it looks leaves this empty, which is
                        // what every tool did before casper existed.
                        shown: output.shown,
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
