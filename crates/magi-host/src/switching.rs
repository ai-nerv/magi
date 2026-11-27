//! Pointing a session at a different model, or a different amount of reasoning. Both rebuild the
//! worker rather than reconfiguring it: it holds a VM built for one protocol, and the level rides
//! on every request the backend makes.

use super::*;

/// Point the session at a different model, or say why not. Returns `None` on success; the new
/// worker is built before the old one is dropped, so a failed switch leaves the session working.
pub(super) async fn switch_model(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    person: &crate::asking::Person,
    scribe: &crate::scribe::Held,
    name: &str,
    persist: bool,
) -> Option<String> {
    let mut boundary = worker.write().await;
    if session.lock().await.busy() {
        return Some("session is busy; change model after queued work finishes".into());
    }
    let Some(mut backend) = catalog.backend(name) else {
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
    if let Some(previous) = boundary.as_ref() {
        backend.grants = previous.held_grants();
    }

    let info = magi_proto::ModelInfo {
        name: backend.model.clone(),
        context_window: backend.context_window.unwrap_or(0),
    };
    let thinking = backend
        .wants
        .thinking
        .map_or("off", magi_model::ThinkingLevel::as_str)
        .to_owned();
    // Taken before the backend is handed to the worker, which consumes it.
    let planning_for = backend
        .context_window
        .map(|window| (backend.model.clone(), window));
    // Gated, like the one it replaces: `Worker::start` is `gated(backend, None)`, which asks nothing.
    let fresh = Arc::new(worker::Worker::gated(
        backend,
        Some(Arc::clone(&person.approver)),
        Arc::clone(&person.asks),
        Arc::clone(&person.holds),
        Arc::clone(&person.knows),
        Arc::clone(scribe),
    ));
    *boundary = Some(fresh);
    {
        let mut held = session.lock().await;
        held.set_model(Some(info.clone()));
        if catalog
            .active_profile_ref()
            .and_then(|p| p.model.as_deref())
            .is_none()
        {
            held.set_base_model(Some(info.name));
        }
        held.set_thinking(thinking);
        // Another model has other providers: a choice made for the last one means nothing here.
        held.set_provider(None);
        held.announce_model();
        if persist {
            remember(catalog, held.model_name(), Some(held.thinking().to_owned()));
        }
    }

    // balthasar is told, because the window it plans against just changed. Told after the switch
    // has taken, so a failed one leaves it planning for the model this session still talks to.
    if let Some((model, window)) = planning_for {
        let told = {
            let mut open = scribe.lock().await;
            match open.as_mut() {
                Some(open) => open.note_model(&model, window).await,
                None => Ok(()),
            }
        };
        if let Err(why) = told {
            magi_model::noted!("scribe: balthasar was not told about the new model: {why}");
        }
    }
    None
}

/// Write down what this directory is now using, so the next run starts with it.
fn remember(catalog: &crate::catalog::Catalog, model: Option<String>, thinking: Option<String>) {
    let cwd = catalog.cwd.display().to_string();
    crate::remember::keep_model(&cwd, model, thinking);
}

/// Ask for more or less reasoning from here on, or say why not. The worker is rebuilt because the
/// level rides on every request, and the worker holds the backend the requests are built from.
pub(super) async fn switch_thinking(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    person: &crate::asking::Person,
    scribe: &crate::scribe::Held,
    level: &str,
) -> Option<String> {
    let mut boundary = worker.write().await;
    if session.lock().await.busy() {
        return Some("session is busy; change thinking after queued work finishes".into());
    }
    let Ok(parsed) = serde_json::from_value::<magi_model::ThinkingLevel>(
        serde_json::Value::String(level.to_owned()),
    ) else {
        return Some(format!(
            "there is no thinking level called {level:?}. \
             Try off, minimal, low, medium, high, xhigh or max."
        ));
    };

    // Rebuilt from the catalog rather than mutated in place, so the level is applied as it would
    // have been had the session started with it.
    let (name, provider) = {
        let held = session.lock().await;
        (held.model_name()?, held.provider().map(ToOwned::to_owned))
    };
    if !catalog
        .thinking_levels(&name)
        .is_some_and(|levels| levels.contains(&parsed))
    {
        return Some(format!(
            "thinking level {level:?} is not selectable for {name}"
        ));
    }
    let mut backend = catalog.backend(&name)?;
    backend.wants.thinking = Some(parsed);
    backend.wants.provider = provider;
    if let Some(previous) = boundary.as_ref() {
        backend.grants = previous.held_grants();
    }
    // Gated, like the one it replaces: `Worker::start` is `gated(backend, None)`, which asks nothing.
    let fresh = Arc::new(worker::Worker::gated(
        backend,
        Some(Arc::clone(&person.approver)),
        Arc::clone(&person.asks),
        Arc::clone(&person.holds),
        Arc::clone(&person.knows),
        Arc::clone(scribe),
    ));
    *boundary = Some(fresh);
    let mut held = session.lock().await;
    held.set_thinking(level.to_owned());
    held.announce_model();
    remember(catalog, held.model_name(), Some(level.to_owned()));
    None
}

/// Have one provider serve the model from here on, or leave it to the router again. Rebuilt the way
/// a thinking change is, keeping the level: both ride on every request.
pub(super) async fn switch_provider(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    person: &crate::asking::Person,
    scribe: &crate::scribe::Held,
    provider: Option<String>,
) -> Option<String> {
    let mut boundary = worker.write().await;
    if session.lock().await.busy() {
        return Some("session is busy; change provider after queued work finishes".into());
    }
    let (name, thinking) = {
        let held = session.lock().await;
        (held.model_name()?, held.thinking().to_owned())
    };
    let mut backend = catalog.backend(&name)?;
    backend.wants.thinking = serde_json::from_value(serde_json::Value::String(thinking)).ok();
    backend.wants.provider.clone_from(&provider);
    if let Some(previous) = boundary.as_ref() {
        backend.grants = previous.held_grants();
    }
    let fresh = Arc::new(worker::Worker::gated(
        backend,
        Some(Arc::clone(&person.approver)),
        Arc::clone(&person.asks),
        Arc::clone(&person.holds),
        Arc::clone(&person.knows),
        Arc::clone(scribe),
    ));
    *boundary = Some(fresh);
    session.lock().await.set_provider(provider);
    None
}

/// How long profile skill preloading may take before timing out cleanly.
const PRELOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Resolve a profile's preloaded skills through the bounded skills helper, returning `(loaded, warnings)`.
pub(crate) async fn preload_skills(
    catalog: &crate::catalog::Catalog,
    names: &[String],
) -> (Vec<(String, String)>, Vec<String>) {
    if names.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let cancel = crate::cancel::Cancel::default();
    let work = async {
        let (client, selected) = match crate::preparing::listing::discover(catalog, &cancel).await {
            Ok(found) => found,
            Err(why) => {
                return (
                    Vec::new(),
                    vec![format!("skills could not be discovered: {why}")],
                );
            }
        };
        let mut loaded = Vec::new();
        let mut warnings = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for name in names {
            if !seen.insert(name.as_str()) {
                continue;
            }
            match selected.load(&client, name, &cancel).await {
                Ok(doc) => loaded.push((doc.document.name, doc.document.body)),
                Err(why) => warnings.push(format!("preloaded skill {name:?} skipped: {why}")),
            }
        }
        (loaded, warnings)
    };
    match tokio::time::timeout(PRELOAD_TIMEOUT, work).await {
        Ok(result) => result,
        Err(_) => {
            cancel.request();
            (
                Vec::new(),
                vec!["preloaded skills timed out; continuing without them".to_owned()],
            )
        }
    }
}

/// Switch the session's active higher-level profile (`""`, `"default"`, `"off"`, or `"none"` clears
/// back to base config). Rebuilds the worker atomically, preserving standing session grants.
pub(super) async fn switch_profile(
    session: &Arc<Mutex<Session>>,
    worker: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    models: &refreshing::Models,
    person: &crate::asking::Person,
    scribe: &crate::scribe::Held,
    name: &str,
) -> Option<String> {
    let requested = name.trim();
    let clearing = matches!(requested, "" | "default" | "off" | "none");
    let snapshot = models.catalog.read().await.clone();
    let target = if clearing {
        None
    } else {
        match snapshot.profiles.iter().find(|p| p.name == requested) {
            Some(found) => Some(found.clone()),
            None => {
                let names: Vec<&str> = snapshot.profiles.iter().map(|p| p.name.as_str()).collect();
                return Some(if names.is_empty() {
                    format!("there is no profile called {requested:?}, and none are configured")
                } else {
                    format!(
                        "there is no profile called {requested:?}. Available: {}",
                        names.join(", ")
                    )
                });
            }
        }
    };

    let (current_model, base_model, current_provider) = {
        let held = session.lock().await;
        (
            held.model_name(),
            held.base_model(),
            held.provider().map(ToOwned::to_owned),
        )
    };
    let fallback_model = base_model
        .filter(|m| snapshot.backend(m).is_some())
        .or_else(|| {
            snapshot
                .base_model
                .clone()
                .filter(|m| snapshot.backend(m).is_some())
        })
        .or_else(|| snapshot.chosen().filter(|m| snapshot.backend(m).is_some()))
        .or_else(|| current_model.clone());
    let model_name = match &target {
        Some(profile) => profile.model.clone().or(fallback_model),
        None => fallback_model,
    };
    if let Some(model) = &model_name
        && snapshot.backend(model).is_none()
    {
        return Some(snapshot.unusable(model).unwrap_or_else(|| {
            format!("profile {requested:?} names model {model:?}, which is not available")
        }));
    }

    let (preloaded, warnings) = match &target {
        Some(profile) if !profile.skills_preload.is_empty() => {
            preload_skills(&snapshot, &profile.skills_preload).await
        }
        _ => (Vec::new(), Vec::new()),
    };

    let mut boundary = worker.write().await;
    if session.lock().await.busy() {
        return Some("session is busy; change profile after queued work finishes".into());
    }

    let updated_catalog = {
        let mut catalog = models.catalog.write().await;
        // Re-verify under the write lock in case a concurrent `models.reload` ran while preloading.
        if let Some(model) = &model_name
            && catalog.backend(model).is_none()
        {
            return Some(catalog.unusable(model).unwrap_or_else(|| {
                format!("profile {requested:?} names model {model:?}, which is no longer available")
            }));
        }
        catalog.active_profile = target.as_ref().map(|p| p.name.clone());
        catalog.skill_allow = target.as_ref().and_then(|p| p.skills_allow.clone());
        catalog.preloaded_skills = preloaded;
        catalog.tool_filter = target.as_ref().map_or(
            magi_tools::ToolFilter::All,
            crate::profiles::Profile::effective_tool_filter,
        );
        catalog.profile_grants = target.as_ref().map_or_else(Vec::new, |p| p.allow.clone());
        catalog.clone()
    };

    let mut planning_for = None;
    let mut new_model_info = None;
    let mut new_thinking = None;
    if let Some(model) = &model_name
        && let Some(mut backend) = updated_catalog.backend(model)
    {
        if current_model.as_deref() == Some(backend.model.as_str()) {
            backend.wants.provider = current_provider;
        }
        if let Some(previous) = boundary.as_ref() {
            backend.grants = previous.held_grants();
        }
        new_model_info = Some(magi_proto::ModelInfo {
            name: backend.model.clone(),
            context_window: backend.context_window.unwrap_or(0),
        });
        new_thinking = Some(
            backend
                .wants
                .thinking
                .map_or("off", magi_model::ThinkingLevel::as_str)
                .to_owned(),
        );
        planning_for = backend
            .context_window
            .map(|window| (backend.model.clone(), window));
        let fresh = Arc::new(worker::Worker::gated(
            backend,
            Some(Arc::clone(&person.approver)),
            Arc::clone(&person.asks),
            Arc::clone(&person.holds),
            Arc::clone(&person.knows),
            Arc::clone(scribe),
        ));
        *boundary = Some(fresh);
    }

    // Always restore base catalog.mode when a profile omits `mode`, so a previous profile's
    // mode never leaks into the next one.
    let target_mode = Some(
        target
            .as_ref()
            .and_then(|profile| profile.mode)
            .unwrap_or(updated_catalog.mode),
    );
    let target_overlay = target.as_ref().map_or_else(
        magi_proto::judging::Rules::default,
        crate::profiles::Profile::judging_rules,
    );
    judging::apply_profile(
        session,
        &person.standing,
        &updated_catalog,
        target_mode,
        target_overlay,
    )
    .await;

    {
        let mut held = session.lock().await;
        if let Some(info) = new_model_info {
            let changed = held.model_name().as_deref() != Some(info.name.as_str());
            held.set_model(Some(info));
            if changed {
                held.set_provider(None);
            }
        }
        if let Some(thinking) = new_thinking {
            held.set_thinking(thinking);
        }
        held.set_profile(target.as_ref().map(|p| p.name.clone()));
        let cwd = updated_catalog.cwd.display().to_string();
        crate::remember::keep_profile(&cwd, target.as_ref().map(|p| p.name.clone()));
        held.announce_model();
        held.announce_profile();
        let cursor = held.cursor();
        let publisher = held.publisher();
        for warning in warnings {
            let _ = publisher.send(HarnessEvent::Noticed {
                cursor,
                text: warning,
            });
        }
        let label = target.as_ref().map_or("default", |p| p.name.as_str());
        let _ = publisher.send(HarnessEvent::Noticed {
            cursor,
            text: format!("profile → {label}"),
        });
    }

    if let Some((model, window)) = planning_for {
        let mut open = scribe.lock().await;
        if let Some(open) = open.as_mut() {
            let _ = open.note_model(&model, window).await;
        }
    }
    None
}

/// Why there is no model, in the terms of the config that produced the situation. Public because
/// the UI says the same thing at attach: a model may well be configured with its provider's key
/// unset while several other providers are ready.
#[must_use]
pub fn no_model(catalog: &crate::catalog::Catalog) -> String {
    let chosen = catalog.chosen();
    let why = chosen.as_deref().and_then(|name| catalog.unusable(name));
    let ready = catalog.usable();

    let mut said = match (&chosen, &why) {
        // The reason already opens with the name, so prefixing it printed the model twice.
        (Some(_), Some(reason)) => format!("{reason}."),
        (Some(name), None) => format!("`{name}` is not a model this build knows about."),
        (None, _) => "No model is configured.".to_owned(),
    };
    if ready.is_empty() {
        said.push_str(" Nothing else is ready either — set a provider key, or run `magi models` to see what each one needs.");
    } else {
        // A few, not all of them: `/model` is one keystroke from here and shows the rest.
        let shown = ready.len().min(3);
        let more = ready.len() - shown;
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        said.push_str(&format!(
            " Ready now: {}{tail}. Type `/model` to switch, or set `magi.model` in your config.",
            ready.get(..shown).unwrap_or(&ready).join(", ")
        ));
    }
    said
}
