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

/// What to set aside for the reply: what was asked for, else the default, and never more than the
/// model can say. Room reserved for an answer that cannot be given is room taken from the
/// conversation, and on a small window it was all of it.
fn reserved(wanted: Option<u64>, cap: Option<u64>) -> u64 {
    let asked = wanted.unwrap_or(REPLY);
    cap.filter(|cap| *cap > 0)
        .map_or(asked, |cap| asked.min(cap))
}

/// What to tell the person when the memory layer stops answering, and when it answers again.
/// `told` is whether they already know: it is said once each way, not on every turn.
fn said_about(told: &mut bool, handed_over: bool) -> Option<&'static str> {
    match (handed_over, *told) {
        (false, false) => {
            *told = true;
            Some(
                "The memory layer has stopped answering, so this conversation is not being \
                 recorded. It carries on, and what is said is kept here and handed over if the \
                 memory layer comes back; if this session ends first, it is lost.",
            )
        }
        (true, true) => {
            *told = false;
            Some("The memory layer is answering again, and the conversation is recorded again.")
        }
        _ => None,
    }
}

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
    /// What helper jobs have cost this prompt, in millionths, shared with every job it started.
    pub spent: crate::helping::Spend,
}

/// The stored cursors of live entries eligible for a provider request.
#[must_use]
pub fn live(session: &Session) -> Vec<u64> {
    let entries = session.entries();
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
        .filter_map(|at| session.cursor_at(at).map(|c| c.0))
        .collect()
}

/// The question put to balthasar before a request.
#[must_use]
pub fn request(
    session: &Session,
    backend: &Backend,
    tools: &[magi_model::Tool],
    round: usize,
) -> serde_json::Value {
    let system = backend
        .system
        .as_deref()
        .map_or(0, magi_model::estimate::tokens);
    let tooling = serde_json::to_string(tools).map_or(0, |t| magi_model::estimate::tokens(&t));
    serde_json::json!({
        "round": round,
        "window": backend.context_window.unwrap_or(0),
        "reply": reserved(backend.wants.max_tokens, backend.max_output),
        "fixed": { "system": system, "tools": tooling },
        "live": live(session),
        "query": crate::context::last_asked(session).unwrap_or_default(),
        "idle_s": session.idle_for().unwrap_or(0),
        "helpers": backend
            .helpers
            .roles
            .keys()
            .filter(|role| !(backend.helpers.no_notes && role.as_str() == "memory"))
            .collect::<Vec<_>>(),
    })
}

/// Build the provider conversation a layout describes. A slot naming an entry that is not live is
/// skipped, and a summary the transcript holds from before layouts is kept if balthasar gave none.
#[must_use]
pub fn render(session: &Session, layout: &Layout) -> Context {
    let entries = session.entries();
    let view = crate::context::live_entries(entries);
    let alive: std::collections::BTreeSet<usize> = view.live.iter().copied().collect();
    let mut built = Built::default();
    if let Some(summary) = &view.summary
        && !layout
            .slots
            .iter()
            .any(|slot| matches!(slot, Slot::Summary { .. }))
    {
        built.observation(summarised(summary));
    }
    for slot in &layout.slots {
        let at = slot
            .cursor()
            .and_then(|cursor| session.position(magi_proto::Cursor(cursor)))
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
            (Slot::Rules { text, .. }, _) if !text.is_empty() => {
                built.user(text.clone());
            }
            (Slot::Observations { text, .. } | Slot::Pinned { text, .. }, _)
                if !text.is_empty() =>
            {
                built.observation(text.clone());
            }
            (Slot::Summary { text, .. }, _) if !text.is_empty() => {
                built.observation(summarised(text))
            }
            (Slot::Note { text, .. }, _) if !text.is_empty() => built.user(text.clone()),
            (Slot::Memory { text, .. }, _) if !text.is_empty() => {
                built.observation(format!(
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
            Slot::Rules { .. } => laid.pinned += 1,
            Slot::Observations { .. } | Slot::Pinned { .. } => laid.notes += 1,
            Slot::Note { .. } => laid.notes += 1,
            Slot::Other => {}
        }
    }
    laid.dropped = live.iter().filter(|c| !named.contains(c)).count();
    laid
}

/// Ask balthasar for a layout, on the clock. One that refuses to lay out is asked for its `plan`
/// instead. `None` for no balthasar, no answer, or nothing usable.
async fn ask(scribe: &crate::scribe::Held, asked: serde_json::Value) -> Option<Layout> {
    tokio::time::timeout(PATIENCE, async {
        let mut open = scribe.lock().await;
        let open = open.as_mut()?;
        match open.layout(asked.clone()).await {
            Ok(answer) => serde_json::from_value(answer)
                .inspect_err(|why| magi_model::noted!("layout: the answer was not a layout: {why}"))
                .ok(),
            Err(magi_ipc::family::Fault::Refused(why)) => {
                magi_model::noted!("layout: refused ({why}); asking for a plan instead");
                let window = asked["window"].as_u64().unwrap_or(0);
                let plan = open.plan_for(window).await.ok()?;
                Some(from_plan(&plan, &cursors(&asked["live"])))
            }
            Err(why) => {
                magi_model::noted!("layout: balthasar could not be asked: {why}");
                None
            }
        }
    })
    .await
    .inspect_err(|_| magi_model::noted!("layout: balthasar did not answer within {PATIENCE:?}"))
    .ok()
    .flatten()
}

/// Cursors, as a list of numbers or of rows that carry one.
fn cursors(list: &serde_json::Value) -> Vec<u64> {
    list.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.as_u64().or_else(|| row.get("cursor")?.as_u64()))
                .collect()
        })
        .unwrap_or_default()
}

/// A `plan`, from a memory layer that does not lay out, as a layout: what it masks goes as its
/// stub, what it drops is left out, everything else is sent. The summary a plan may want is not
/// written: summaries are helper jobs, and a plan has no way to hand one out.
#[must_use]
pub fn from_plan(plan: &serde_json::Value, live: &[u64]) -> Layout {
    let masks: std::collections::BTreeMap<u64, String> = plan["mask"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| Some((row["cursor"].as_u64()?, row["as"].as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let dropped: std::collections::BTreeSet<u64> = cursors(&plan["drop"]).into_iter().collect();
    let slots = live
        .iter()
        .filter(|cursor| !dropped.contains(cursor))
        .map(|&cursor| match masks.get(&cursor) {
            Some(text) => Slot::Stub {
                cursor,
                text: text.clone(),
            },
            None => Slot::Item { cursor },
        })
        .collect();
    Layout {
        id: String::new(),
        budget: serde_json::Value::Null,
        slots,
        jobs: Vec::new(),
        fits: plan["fits"].as_bool().unwrap_or(true),
        why: format!(
            "balthasar does not lay out; its plan: {}",
            plan["why"].as_str().unwrap_or("no reason given")
        ),
    }
}

/// Each slot as a screen lists it: its kind, its entry, what it costs, and a line of what it is.
#[must_use]
pub fn listed(session: &Session, layout: &Layout) -> Vec<magi_proto::laying::LaidSlot> {
    let entries = session.entries();
    let line = |text: &str| -> String {
        text.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default()
            .chars()
            .take(90)
            .collect()
    };
    let costs = |said: u64, text: &str| {
        if said > 0 {
            said
        } else {
            magi_model::estimate::tokens(text)
        }
    };
    layout
        .slots
        .iter()
        .filter_map(|slot| {
            let (kind, cursor, spent, text) = match slot {
                Slot::Item { cursor } => {
                    let entry = entries.get(session.position(magi_proto::Cursor(*cursor))?)?;
                    let spent = crate::scribe::tokens(entry);
                    ("item", Some(*cursor), spent, line(&described(entry)))
                }
                Slot::Stub { cursor, text } => ("stub", Some(*cursor), costs(0, text), line(text)),
                Slot::Rules { text, tokens } => ("rules", None, costs(*tokens, text), line(text)),
                Slot::Observations { text, tokens } | Slot::Pinned { text, tokens } => {
                    ("observations", None, costs(*tokens, text), line(text))
                }
                Slot::Summary { text, tokens } => {
                    ("summary", None, costs(*tokens, text), line(text))
                }
                Slot::Note { text, tokens } => ("note", None, costs(*tokens, text), line(text)),
                Slot::Memory { text, tokens, .. } => {
                    ("memory", None, costs(*tokens, text), line(text))
                }
                Slot::Other => return None,
            };
            Some(magi_proto::laying::LaidSlot {
                kind: kind.to_owned(),
                cursor,
                tokens: spent,
                text,
            })
        })
        .collect()
}

/// Who said an entry, and the start of what.
fn described(entry: &Entry) -> String {
    match entry {
        Entry::User { text, .. } => format!("you: {text}"),
        Entry::From { who, text, .. } => format!("{who}: {text}"),
        Entry::Assistant { text, thinking, .. } if text.trim().is_empty() => {
            format!("model thinking: {thinking}")
        }
        Entry::Assistant { text, .. } => format!("model: {text}"),
        Entry::Tool { name, args, .. } => format!("{name} {args}"),
        _ => String::new(),
    }
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
    let handed = crate::scribe::flush(session, &mut *scribe.lock().await).await;
    if let Err(why) = &handed {
        magi_model::noted!("layout: the transcript could not be handed over: {why}");
    }
    {
        let mut held = session.lock().await;
        if let Some(text) = said_about(&mut held.unrecorded, handed.is_ok()) {
            let _ = held.publisher().send(HarnessEvent::Noticed {
                cursor: magi_proto::Cursor::ZERO,
                text: text.to_owned(),
            });
        }
    }
    let asked = request(&*session.lock().await, backend, tools, prompt.round);
    magi_model::noted!(
        "layout: asking for round {} of {} live entries in a {} window",
        prompt.round,
        asked["live"].as_array().map_or(0, Vec::len),
        asked["window"]
    );
    let mut layout = if backend.context_window.is_some() {
        ask(scribe, asked.clone()).await
    } else {
        None
    };

    // A blocking job changes the answer, so it runs and the question is put once more. The rest
    // start now, beside the turn: a rule just stated is noted before an agent this turn starts.
    if let Some(first) = layout.as_mut() {
        let (blocking, background): (Vec<_>, Vec<_>) = std::mem::take(&mut first.jobs)
            .into_iter()
            .partition(|j| j.blocking);
        let events = session.lock().await.publisher();
        let helpers = session.lock().await.helpers();
        crate::helping::alongside(
            &helpers,
            background,
            backend.clone(),
            std::sync::Arc::clone(scribe),
            events.clone(),
            std::sync::Arc::clone(&prompt.spent),
        );
        if !blocking.is_empty() {
            if let Err(why) =
                crate::helping::work(&blocking, backend, scribe, &events, &prompt.spent).await
            {
                magi_model::noted!("helpers: {why}");
            }
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
    let live = live(&held);
    let mut layout = match layout {
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
    held.defer(std::mem::take(&mut layout.jobs));
    if let Some(injection) = layout.slots.iter().find_map(|slot| match slot {
        Slot::Memory { injection, .. } => injection.clone(),
        _ => None,
    }) {
        prompt.injection = Some(injection);
    }
    let context = render(&held, &layout);
    let counted = counts(&layout, &live);
    magi_model::noted!(
        "layout: {} — {} whole, {} stubbed, {} left out, {} summary — {}",
        if layout.id.is_empty() {
            "magi's"
        } else {
            &layout.id
        },
        counted.items,
        counted.stubs,
        counted.dropped,
        counted.summary,
        layout.why
    );
    let _ = held.publisher().send(HarnessEvent::ContextLaid {
        id: layout.id.clone(),
        budget: layout.budget.clone(),
        counts: counted,
        why: layout.why.clone(),
        slots: listed(&held, &layout),
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
