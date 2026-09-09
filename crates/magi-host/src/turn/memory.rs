//! What the turn does about memory: compaction, and what balthasar is asked and told. Each is best
//! effort on the turn's own clock, and a session with no balthasar behaves as it did before one.

use super::Backend;
use crate::session::Session;
use magi_proto::{AgentStatus, Entry, MessageId};
use magi_tools::Registry;

/// Do what balthasar's plan says, and journal it. Masking first, always — it is free and reversible
/// — and only what masking could not free is summarised. balthasar marks a turn masked as it hands
/// the plan over and never offers it again, so a mask not applied leaves it planning against a
/// fiction. Returns whether anything was summarised, which is the one the caller retries on.
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

    // balthasar says how much; `crate::compact::legal` says where that cut may actually fall.
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

    // The messages of exactly the entries being replaced. Entry counts and message counts agree
    // only when every entry makes one message, and a `Notice`, `Branch` or `Compaction` makes none.
    let asked = crate::compact::request(&crate::context::of_entries(&entries[..covered]));
    let mut turn = magi_core::Turn::new();
    let mut deltas = Vec::new();
    // Collected rather than streamed: nobody watches a compaction.
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
    // Compaction runs between turns; a watcher that wants what it took out has to be told now.
    if committed.is_ok() {
        registry.saw(&magi_tools::Event::Compacted {
            dropped: covered,
            kept,
        });
    }
    committed.is_ok()
}

/// How long a recall may hold up a turn, so the turn is independent of how quick balthasar is.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(250);

/// What balthasar said to do with the window, held as its own type: the plan is consulted three
/// times, and reaching into `serde_json::Value` is three chances to spell a key wrong.
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

/// What balthasar says to do, or nothing when it has nothing to say. Cursors count from one and
/// entry indices from zero, so cursor `c` names entry `c - 1`, and `summarise.to` is exactly the
/// count of entries covered — both conversions happen here, once. A balthasar that has observed
/// nothing refuses, and a session with no scribe is not planned for at all.
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

    // A plan that does not fit is said out loud: balthasar reserves room for the answer and the
    // injection, and a window smaller than that reserve leaves it nothing to plan with.
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

    // A mask with no text is skipped rather than sent empty: balthasar leaves alone a tool it
    // cannot describe, so an entry here with nothing to say is a shape nobody meant.
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

/// Write down what the plan said to stub, so the next context is built with it. balthasar never
/// offers a mask twice, so one applied without being recorded means full text for the rest of the
/// session. A mask naming an entry that is not a tool result is dropped.
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

/// What this project remembers about the prompt in front of it, as a message. Keyed on the last
/// thing the person said. Best effort: a balthasar missing, wedged or refusing costs the turn nothing.
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

    // On a clock, because this sits in front of the person's turn.
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

    // Every supplier that has something to say, packed together — see `crate::supplying`.
    let offers = vec![crate::injecting::offered("balthasar", &found.memories)];
    let packed = crate::supplying::pack(&offers, window);

    // What did not fit is said out loud rather than dropped in silence.
    for dropped in &packed.dropped {
        magi_model::noted!(
            "memory: {} offered something the budget would not take: {}",
            dropped.from,
            dropped.text.chars().take(60).collect::<String>()
        );
    }
    let message = packed.message;
    // The id travels with the message: balthasar can only attribute an outcome against the
    // injection it served the memories under. The cost is the half of the price nothing recorded.
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

/// Report one finished tool against the injection that preceded it. The action is one string,
/// because that is what balthasar hashes; the arguments do not leave this process. `recall` and
/// `remember` are skipped, or every injection would look used.
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

    // On the same clock as the recall: this is instrumentation.
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
