//! Pointing a session at a different model, or a different amount of reasoning.
//!
//! Split out under THE RULE; the session next door is what these are for. Both rebuild the
//! worker rather than reconfiguring it: the worker holds a VM built for one protocol, and the
//! level rides on every request the backend makes.

use super::*;

/// Point the session at a different model, or say why not.
///
/// Returns `None` on success. The new worker is built before the old one is dropped, so a
/// switch that fails leaves the session able to carry on with what it had.
pub(super) async fn switch_model(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    person: &crate::asking::Person,
    scribe: &crate::scribe::Held,
    name: &str,
) -> Option<String> {
    let Some(backend) = catalog.backend(name) else {
        return Some(catalog.unusable(name).unwrap_or_else(|| {
            let usable = catalog.usable();
            if usable.is_empty() {
                format!("there is no model called {name:?}, and none is configured")
            } else {
                format!(
                    "there is no model called {name:?}. Available: {}",
                    usable.join(", ")
                )
            }
        }));
    };

    let info = magi_proto::ModelInfo {
        name: backend.model.clone(),
        context_window: backend.context_window.unwrap_or(0),
    };
    // Gated, like the one it replaces. `Worker::start` is `gated(backend, None)` — a worker
    // nothing asks — so switching the model used to switch the permission model off with it,
    // and every tool for the rest of the session ran without being asked about.
    let fresh = Arc::new(worker::Worker::gated(
        backend,
        Some(Arc::clone(&person.approver)),
        Arc::clone(&person.asks),
        Arc::clone(&person.holds),
        Arc::clone(scribe),
    ));
    *worker.write().await = Some(fresh);
    {
        let mut held = session.lock().await;
        held.set_model(Some(info));
        // Announced so the footer changes now rather than after the next turn: the whole
        // point of switching is to see that it happened.
        held.announce_model();
        remember(catalog, held.model_name(), Some(held.thinking().to_owned()));
    }
    None
}

/// Write down what this directory is now using, so the next run starts with it.
///
/// A switch made in the UI is a decision somebody made in front of the thing. Forgetting it on
/// restart meant the only way to keep a choice was to stop making it in the UI and edit a file.
fn remember(catalog: &crate::catalog::Catalog, model: Option<String>, thinking: Option<String>) {
    let cwd = catalog.cwd.display().to_string();
    crate::remember::keep(&cwd, &crate::remember::Chosen { model, thinking });
}

/// Ask for more or less reasoning from here on, or say why not.
///
/// The worker is rebuilt for the same reason a model switch rebuilds it: the level rides on
/// every request, and the worker holds the backend the requests are built from.
pub(super) async fn switch_thinking(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    person: &crate::asking::Person,
    scribe: &crate::scribe::Held,
    level: &str,
) -> Option<String> {
    let Ok(parsed) = serde_json::from_value::<magi_model::ThinkingLevel>(
        serde_json::Value::String(level.to_owned()),
    ) else {
        return Some(format!(
            "there is no thinking level called {level:?}. \
             Try off, minimal, low, medium, high or max."
        ));
    };

    // Rebuilt from the catalog rather than mutated in place, so the level is applied the same
    // way it would have been had the session started with it.
    let name = session.lock().await.model_name()?;
    let mut backend = catalog.backend(&name)?;
    backend.wants.thinking = Some(parsed);
    // Gated, like the one it replaces. `Worker::start` is `gated(backend, None)` — a worker
    // nothing asks — so switching the model used to switch the permission model off with it,
    // and every tool for the rest of the session ran without being asked about.
    let fresh = Arc::new(worker::Worker::gated(
        backend,
        Some(Arc::clone(&person.approver)),
        Arc::clone(&person.asks),
        Arc::clone(&person.holds),
        Arc::clone(scribe),
    ));
    *worker.write().await = Some(fresh);
    let mut held = session.lock().await;
    held.set_thinking(level.to_owned());
    held.announce_model();
    remember(catalog, held.model_name(), Some(level.to_owned()));
    None
}

/// Why there is no model, in the terms of the config that produced the situation.
///
/// Public because the UI says the same thing at attach, and said it worse: a fixed "No model is
/// configured" on screen while the daemon, on the first prompt, gave the real reason. Two answers
/// to one question, and the one you met first was the wrong one.
///
/// "No model is configured" was the whole of what this said, and it was wrong in the common
/// case: a model *is* configured, its provider's key is not set, and several other providers
/// are ready and waiting. Somebody whose environment holds an OpenRouter key reads that message
/// as OpenRouter being broken, because nothing in it mentions either fact.
#[must_use]
pub fn no_model(catalog: &crate::catalog::Catalog) -> String {
    let chosen = catalog.chosen();
    let why = chosen.as_deref().and_then(|name| catalog.unusable(name));
    let ready = catalog.usable();

    let mut said = match (&chosen, &why) {
        // The reason already opens with the name, so prefixing it printed the model twice in
        // one sentence.
        (Some(_), Some(reason)) => format!("{reason}."),
        (Some(name), None) => format!("`{name}` is not a model this build knows about."),
        (None, _) => "No model is configured.".to_owned(),
    };
    if ready.is_empty() {
        said.push_str(" Nothing else is ready either — set a provider key, or run `magi models` to see what each one needs.");
    } else {
        // A few, not all of them. Nine model ids is a wall of text in an error, and the point
        // is to get moving: `/model` is one keystroke from here and shows the rest.
        let shown = ready.len().min(3);
        let more = ready.len() - shown;
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        said.push_str(&format!(
            " Ready now: {}{tail}. Type `/model` to switch, or set `magi.model` in your config.",
            ready[..shown].join(", ")
        ));
    }
    said
}
