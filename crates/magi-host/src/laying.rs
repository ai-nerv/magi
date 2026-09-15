//! What goes into each request, as balthasar lays it out. magi asks, renders what it is handed and
//! says what the provider made of it; the deciding is balthasar's. With no answer the last layout
//! grows by what is new since, and with no layout at all everything live is sent.

use crate::catalog::Backend;
use crate::context::Built;
use crate::session::Session;
use magi_model::Context;
use magi_proto::{Entry, HarnessEvent, Laid};

pub use magi_proto::laying::{Layout, Slot};

/// How long a layout may hold up a request. Longer than a recall was allowed: nothing is sent
/// without one, and a balthasar that must read the whole transcript takes its time.
pub const PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a report about a request already sent may take. Instrumentation, on a short clock.
const REPORT: std::time::Duration = std::time::Duration::from_millis(500);

/// The answer a request leaves room for when nothing configured says how long one may be.
const REPLY: u64 = 32_000;

/// How many tighter layouts one prompt may ask for after the provider refused one as too long.
pub const OVERFLOWS: u8 = 3;

/// What one prompt has been through so far, across its rounds.
#[derive(Debug, Default)]
pub struct Prompt {
    /// Which request of the prompt this is, from zero.
    pub round: usize,
    /// How many times the provider has refused a layout as too long.
    pub overflows: u8,
    /// The layout the request in flight was built from, when balthasar made it.
    pub id: String,
    /// The ledger entry the memory in this prompt was served under, for the outcome report.
    pub injection: Option<String>,
    /// What helper jobs have cost this prompt, in millionths.
    pub spent: u64,
}

/// The cursors a request could send: what is live, less what is never sent. Cursor `c` is entry
/// `c - 1`, as everywhere balthasar is spoken to.
#[must_use]
pub fn live(entries: &[Entry]) -> Vec<u64> {
    crate::context::live_entries(entries)
        .live
        .into_iter()
        .filter(|&at| match &entries[at] {
            Entry::Notice { .. } => false,
            Entry::Assistant {
                error, stop_reason, ..
            } => error.is_none() && *stop_reason != Some(magi_model::StopReason::Error),
            _ => true,
        })
        .map(|at| at as u64 + 1)
        .collect()
}

/// A token per four characters, the estimate balthasar corrects against.
fn tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

/// The question put to balthasar before a request.
#[must_use]
pub fn request(
    session: &Session,
    backend: &Backend,
    tools: &[magi_model::Tool],
    round: usize,
) -> serde_json::Value {
    let system = backend.system.as_deref().map_or(0, |s| s.chars().count());
    let tooling = serde_json::to_string(tools).map_or(0, |t| t.chars().count());
    serde_json::json!({
        "round": round,
        "window": backend.context_window.unwrap_or(0),
        "reply": backend.wants.max_tokens.unwrap_or(REPLY),
        "fixed": { "system": tokens(system), "tools": tokens(tooling) },
        "live": live(session.entries()),
        "query": crate::context::last_asked(session).unwrap_or_default(),
        "idle_s": session.idle_for().unwrap_or(0),
        "helpers": backend.helpers.roles.keys().collect::<Vec<_>>(),
    })
}

/// Build the provider conversation a layout describes. A slot naming an entry that is not live is
/// skipped, and a summary the transcript holds from before layouts is kept if balthasar gave none.
#[must_use]
pub fn render(entries: &[Entry], layout: &Layout) -> Context {
    let view = crate::context::live_entries(entries);
    let alive: std::collections::BTreeSet<usize> = view.live.iter().copied().collect();
    let mut built = Built::default();
    if let Some(summary) = &view.summary
        && !layout
            .slots
            .iter()
            .any(|slot| matches!(slot, Slot::Summary { .. }))
    {
        built.user(summarised(summary));
    }
    for slot in &layout.slots {
        let at = slot
            .cursor()
            .and_then(|cursor| usize::try_from(cursor).ok()?.checked_sub(1))
            .filter(|at| alive.contains(at));
        match (slot, at) {
            (Slot::Item { .. }, Some(at)) => {
                built.entry(&entries[at], view.masks.get(&at).map(String::as_str));
            }
            (Slot::Stub { text, .. }, Some(at)) => {
                let stub = Some(text.as_str()).filter(|t| !t.is_empty());
                built.entry(
                    &entries[at],
                    stub.or_else(|| view.masks.get(&at).map(String::as_str)),
                );
            }
            (Slot::Pinned { text }, _) if !text.is_empty() => {
                built.user(format!("Pinned notes for this project:\n\n{text}"));
            }
            (Slot::Summary { text }, _) if !text.is_empty() => built.user(summarised(text)),
            (Slot::Note { text }, _) if !text.is_empty() => built.user(text.clone()),
            (Slot::Memory { text, .. }, _) if !text.is_empty() => {
                built.user(format!(
                    "From memory — what this project recorded before; check it before relying \
                     on it:\n\n{text}"
                ));
            }
            _ => {}
        }
    }
    built.finish()
}

fn summarised(summary: &str) -> String {
    format!("Here is a summary of the earlier part of this conversation:\n\n{summary}")
}

/// The last layout, grown by what is live and newer than anything it named: what is sent when
/// balthasar does not answer. What it dropped stays dropped, and it has no id to report against.
#[must_use]
pub fn extend(last: &Layout, live: &[u64]) -> Layout {
    let alive: std::collections::BTreeSet<u64> = live.iter().copied().collect();
    let mut slots: Vec<Slot> = last
        .slots
        .iter()
        .filter(|slot| slot.cursor().is_none_or(|c| alive.contains(&c)))
        .cloned()
        .collect();
    let high = slots.iter().filter_map(Slot::cursor).max().unwrap_or(0);
    slots.extend(
        live.iter()
            .filter(|&&c| c > high)
            .map(|&cursor| Slot::Item { cursor }),
    );
    Layout {
        id: String::new(),
        budget: last.budget.clone(),
        slots,
        jobs: Vec::new(),
        fits: last.fits,
        why: "balthasar did not answer: the last layout, and what is new since".to_owned(),
    }
}

/// Everything live, as a layout: what is sent when there has never been one.
#[must_use]
pub fn whole(live: &[u64]) -> Layout {
    Layout {
        id: String::new(),
        budget: serde_json::Value::Null,
        slots: live.iter().map(|&cursor| Slot::Item { cursor }).collect(),
        jobs: Vec::new(),
        fits: true,
        why: "no layout: everything live is sent".to_owned(),
    }
}

/// Whether a layout can be sent as it stands: it names something live, and the last live entry —
/// the prompt being answered — is among what it names.
#[must_use]
pub fn sound(layout: &Layout, live: &[u64]) -> bool {
    let Some(last) = live.last() else {
        return true;
    };
    layout.slots.iter().any(|slot| slot.cursor() == Some(*last))
}

/// How a layout comes out, for the model card and the `:context` view.
#[must_use]
pub fn counts(layout: &Layout, live: &[u64]) -> Laid {
    let mut laid = Laid::default();
    let mut named = std::collections::BTreeSet::new();
    for slot in &layout.slots {
        match slot {
            Slot::Item { cursor } => {
                laid.items += 1;
                named.insert(*cursor);
            }
            Slot::Stub { cursor, .. } => {
                laid.stubs += 1;
                named.insert(*cursor);
            }
            Slot::Summary { .. } => laid.summary += 1,
            Slot::Memory { .. } => laid.memory += 1,
            Slot::Pinned { .. } => laid.pinned += 1,
            Slot::Note { .. } => laid.notes += 1,
            Slot::Other => {}
        }
    }
    laid.dropped = live.iter().filter(|c| !named.contains(c)).count();
    laid
}

/// Ask balthasar for a layout, on the clock. `None` for no balthasar, a refusal, or no answer.
async fn ask(scribe: &crate::scribe::Held, asked: serde_json::Value) -> Option<Layout> {
    let answered = tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        open.as_mut()?
            .layout(asked)
            .await
            .inspect_err(|why| magi_model::noted!("layout: balthasar refused: {why}"))
            .ok()
    })
    .await
    .inspect_err(|_| magi_model::noted!("layout: balthasar did not answer within {PATIENCE:?}"))
    .ok()
    .flatten()?;
    serde_json::from_value(answered)
        .inspect_err(|why| magi_model::noted!("layout: the answer was not a layout: {why}"))
        .ok()
}

/// Lay out the next request of a prompt: hand balthasar everything settled, ask it, run whatever it
/// must have first, and build what it said. Never fails: with nothing to ask, everything is sent.
pub async fn lay(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    tools: &[magi_model::Tool],
    scribe: &crate::scribe::Held,
    prompt: &mut Prompt,
) -> Context {
    // Working from here, not from the provider call: balthasar can take its time answering.
    session
        .lock()
        .await
        .set_status(magi_proto::AgentStatus::Working {
            label: "Laying out".into(),
        });
    // What balthasar lays out is what it holds, so everything settled goes first.
    if let Err(why) = crate::scribe::flush(session, &mut *scribe.lock().await).await {
        magi_model::noted!("layout: the transcript could not be handed over: {why}");
    }
    let asked = request(&*session.lock().await, backend, tools, prompt.round);
    let mut layout = if backend.context_window.is_some() {
        ask(scribe, asked.clone()).await
    } else {
        None
    };

    // A blocking job changes the answer, so it runs and the question is put once more.
    if let Some(first) = &layout {
        let blocking: Vec<_> = first.jobs.iter().filter(|j| j.blocking).cloned().collect();
        if !blocking.is_empty() {
            let events = session.lock().await.publisher();
            crate::helping::work(&blocking, backend, scribe, &events, &mut prompt.spent).await;
            if let Some(again) = ask(scribe, asked).await {
                layout = Some(again);
            }
        }
    }
    settle(session, layout, prompt).await
}

/// After the provider refused the request as too long: a tighter layout, built, or `None` when
/// there is nobody to ask for one.
pub async fn overflowed(
    session: &tokio::sync::Mutex<Session>,
    scribe: &crate::scribe::Held,
    prompt: &mut Prompt,
    said: &str,
) -> Option<Context> {
    if prompt.id.is_empty() || prompt.overflows >= OVERFLOWS {
        return None;
    }
    prompt.overflows += 1;
    let id = prompt.id.clone();
    let answered = tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        open.as_mut()?.overflowed(&id, said).await.ok()
    })
    .await
    .ok()
    .flatten()?;
    let layout: Layout = serde_json::from_value(answered).ok()?;
    Some(settle(session, Some(layout), prompt).await)
}

/// Build what a layout says — or, with none, the last one grown, or everything — remember it, and
/// tell whoever is watching how it came out.
async fn settle(
    session: &tokio::sync::Mutex<Session>,
    layout: Option<Layout>,
    prompt: &mut Prompt,
) -> Context {
    let mut held = session.lock().await;
    let live = live(held.entries());
    let layout = match layout {
        Some(layout) if sound(&layout, &live) => layout,
        Some(layout) => {
            magi_model::noted!(
                "layout: {} leaves out the prompt; sending the last one",
                layout.id
            );
            fallback(held.laid(), &live)
        }
        None => fallback(held.laid(), &live),
    };
    prompt.id.clone_from(&layout.id);
    if let Some(injection) = layout.slots.iter().find_map(|slot| match slot {
        Slot::Memory { injection, .. } => injection.clone(),
        _ => None,
    }) {
        prompt.injection = Some(injection);
    }
    let context = render(held.entries(), &layout);
    let _ = held.publisher().send(HarnessEvent::ContextLaid {
        id: layout.id.clone(),
        budget: layout.budget.clone(),
        counts: counts(&layout, &live),
        why: layout.why.clone(),
    });
    held.lay(layout);
    context
}

fn fallback(last: Option<&Layout>, live: &[u64]) -> Layout {
    last.map_or_else(|| whole(live), |last| extend(last, live))
}

/// Tell balthasar the provider took the request, and what it counted — what corrects its estimate.
pub async fn applied(scribe: &crate::scribe::Held, prompt: &Prompt, usage: magi_proto::Usage) {
    if prompt.id.is_empty() {
        return;
    }
    let reported = tokio::time::timeout(REPORT, async {
        let mut open = scribe.lock().await;
        if let Some(open) = open.as_mut()
            && let Err(why) = open.applied(&prompt.id, usage).await
        {
            magi_model::noted!("layout: applied was refused: {why}");
        }
    })
    .await;
    if reported.is_err() {
        magi_model::noted!("layout: applied did not land within {REPORT:?}");
    }
}

#[cfg(test)]
#[path = "laying/tests.rs"]
mod tests;
