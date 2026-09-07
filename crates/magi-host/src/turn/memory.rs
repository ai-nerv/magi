//! What the turn does about memory: compaction, and what balthasar is asked and told.
//!
//! Split out under THE RULE; the loop that calls all of this is next door. They belong together
//! because each is the same shape — best effort, on the turn's own clock, and a session with no
//! balthasar behaves exactly as it did before there was one.

use super::Backend;
use crate::session::Session;
use magi_proto::{AgentStatus, Entry, MessageId};
use magi_tools::Registry;

/// Summarise the earlier part of the conversation and journal the result.
///
/// Returns whether anything was compacted. A failure is not fatal: the turn goes ahead with
/// the context it has and either fits or is refused by the provider, which is no worse than
/// not having tried. Losing the conversation because the summariser had a bad minute would be.
pub(super) async fn compact(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    registry: &Registry,
) -> bool {
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
    let kept = held.entries().len().saturating_sub(covered);
    let committed = held.commit(Entry::Compaction {
        id,
        summary: turn.text().trim().to_owned(),
        replaces: covered,
    });
    // **The one thing that happens to a session with no other way to observe it.** Compaction
    // runs between turns and leaves a summary behind; what it took out is gone from the window
    // and named nowhere. A watcher that wants to keep it has to be told at the moment.
    if committed.is_ok() {
        registry.saw(&magi_tools::Event::Compacted {
            dropped: covered,
            kept,
        });
    }
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
pub(super) async fn second_opinion(
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
pub(super) async fn remembered(
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
pub(super) async fn acted_on(
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
