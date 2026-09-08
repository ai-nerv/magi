//! What the turn does about memory: compaction, and what balthasar is asked and told.
//!
//! Split out under THE RULE; the loop that calls all of this is next door. They belong together
//! because each is the same shape — best effort, on the turn's own clock, and a session with no
//! balthasar behaves exactly as it did before there was one.

use super::Backend;
use crate::session::Session;
use magi_proto::{AgentStatus, Entry, MessageId};
use magi_tools::Registry;

/// Do what balthasar's plan says, and journal it.
///
/// **The whole ladder, in balthasar's order.** Masking first, always: it is free, it is reversible
/// — the text is still in balthasar's scratch and still on this session's screen — and tool output
/// is most of a coding session's window. Only what masking could not free is summarised, because a
/// summary costs a request of its own and loses detail nothing can get back.
///
/// magi obeyed only the summary for a while, which was worse than not obeying at all: balthasar
/// marks a turn masked as it hands the plan over and never offers it again, costing it as a stub
/// in every later plan — so magi sent the full text for the rest of the session while balthasar
/// planned against a fiction.
///
/// Returns whether anything was *summarised*, which is the one the caller retries on. A failure is
/// not fatal: the turn goes ahead with the context it has and either fits or is refused by the
/// provider, which is no worse than not having tried.
pub(super) async fn compact(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    registry: &Registry,
    scribe: &crate::scribe::Held,
) -> bool {
    let entries = {
        let held = session.lock().await;
        held.entries().to_vec()
    };
    let Some(plan) = planned(backend, scribe, entries.len()).await else {
        return false;
    };
    masked(session, &plan).await;

    // balthasar says how much; `legal` says where that cut may actually fall. Not a second
    // opinion about the amount — the one thing balthasar cannot know, because it is a fact about
    // the provider wire rather than about the conversation. See `crate::compact`.
    let Some(covered) = plan
        .summarises()
        .and_then(|asked| crate::compact::legal(&entries, asked))
    else {
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

/// What balthasar said to do with the window.
///
/// Held as its own type rather than read out of the JSON at each use, because the plan is
/// consulted three times — masks, summary, and the reason for the log — and a caller reaching into
/// `serde_json::Value` three times is three chances to spell a key wrong and get `None`.
struct Plan {
    /// Tool results to send as a stub, by entry index, with what to send.
    masks: Vec<(usize, String)>,
    /// How many entries a summary would replace, when balthasar wants one.
    summarise: Option<usize>,
}

impl Plan {
    /// How many entries at the front to summarise, if balthasar asked for that at all.
    const fn summarises(&self) -> Option<usize> {
        self.summarise
    }
}

/// What balthasar says to do, or nothing when it has nothing to say.
///
/// **The decision, taken where the memory layer is.** magi used to make this itself — a constant
/// `KEEP = 8` and a character estimate — then ask balthasar what *it* would do, write the
/// difference to a debug log, and go ahead with its own answer anyway. Two deciders disagreeing in
/// a line nobody read.
///
/// Cursors count from one and magi's entry indices from zero, so a cursor `c` names entry `c - 1`
/// — and `summarise.to`, being the *last* cursor covered, is exactly the *count* of entries
/// covered. Both conversions happen here, once, so nothing downstream has to hold two spaces in
/// mind at the same time.
///
/// Best effort, on the same clock as everything else here. A balthasar that has observed nothing
/// refuses this, which is what a harness that has not streamed its turns gets; a session with no
/// scribe at all is not planned for at all, which is correct rather than a gap — there is no
/// second opinion to fall back to, and inventing one here is the thing being removed.
async fn planned(backend: &Backend, scribe: &crate::scribe::Held, entries: usize) -> Option<Plan> {
    let window = backend.context_window?;
    let plan = tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        open.as_mut()?.plan_for(window).await.ok()
    })
    .await
    .inspect_err(|_| magi_model::noted!("compact: balthasar did not plan within {PATIENCE:?}"))
    .ok()
    .flatten()?;

    // **A plan that does not fit is said out loud rather than passed over.** balthasar reserves
    // room for the answer and the injection, and a window smaller than that reserve leaves it
    // nothing to plan with — it says so in `why` and offers no span. Silently doing nothing is
    // then indistinguishable from having nothing to do, and the session fills up with nobody able
    // to say which of the two happened.
    let why = plan
        .get("why")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("no reason given");
    if !plan
        .get("fits")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
    {
        magi_model::noted!("compact: balthasar cannot plan for this window — {why}");
    }

    // A mask names the cursor it applies to and carries the stub the *tool's own* handler wrote.
    // One without text is skipped rather than sent empty: balthasar leaves a tool it cannot
    // describe alone, so an entry here with nothing to say is a shape nobody meant.
    let masks: Vec<(usize, String)> = plan
        .get("mask")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let cursor = row.get("cursor").and_then(serde_json::Value::as_u64)?;
                    let shown = row.get("as").and_then(serde_json::Value::as_str)?;
                    let at = usize::try_from(cursor).ok()?.checked_sub(1)?;
                    (!shown.is_empty()).then(|| (at, shown.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();

    let summarise = plan
        .get("summarise")
        .and_then(|span| span.get("to"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|to| usize::try_from(to).ok());

    if !masks.is_empty() || summarise.is_some() {
        magi_model::noted!(
            "compact: balthasar masks {} and summarises {} of {entries} entries — {why}",
            masks.len(),
            summarise.unwrap_or(0)
        );
    }
    Some(Plan { masks, summarise })
}

/// Write down what the plan said to stub, so the next context is built with it.
///
/// **Recorded, not merely applied.** balthasar marks a turn masked as it hands the plan over and
/// never offers it again; a magi that stubbed a result without writing it down would send the full
/// text for the rest of the session while balthasar planned against a stub.
///
/// A mask naming an entry that is not a tool result is dropped. balthasar only ever masks tool
/// output — that is where a coding session's tokens are — and a stub on anything else would be
/// magi inventing a substitution nobody asked for.
async fn masked(session: &tokio::sync::Mutex<Session>, plan: &Plan) {
    if plan.masks.is_empty() {
        return;
    }
    let mut held = session.lock().await;
    for (at, shown) in &plan.masks {
        if !matches!(
            held.entries().get(*at),
            Some(Entry::Tool {
                result: Some(_),
                ..
            })
        ) {
            magi_model::noted!("compact: a mask for entry {at}, which is not a tool result");
            continue;
        }
        let id = MessageId::new(format!("m{}", held.cursor().next().0));
        let _ = held.commit(Entry::Masked {
            id,
            at: *at,
            shown: shown.clone(),
        });
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

    // **Every supplier that has something to say, packed together.** One entry today; the shape
    // is what makes the second one an entry rather than a rewrite — see `crate::supplying`.
    let offers = vec![crate::injecting::offered("balthasar", &found.memories)];
    let packed = crate::supplying::pack(&offers, window);

    // **What did not fit is said out loud.** The renderer this replaces stopped writing when the
    // budget ran out and told nobody, so a supplier whose answers were all slightly too long was
    // indistinguishable from one that found nothing.
    for dropped in &packed.dropped {
        magi_model::noted!(
            "memory: {} offered something the budget would not take: {}",
            dropped.from,
            dropped.text.chars().take(60).collect::<String>()
        );
    }
    let message = packed.message;
    // The id travels with the message. It is what makes an outcome attributable later: balthasar
    // decides for itself whether an action followed any of the memories it gave, and it can only
    // do that against the injection it served them under.
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
