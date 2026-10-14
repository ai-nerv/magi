//! Transient session variables and export-only script imports.

mod parsing;

use crate::{Session, asking::Person, refreshing::Models, worker::Worker};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub(super) async fn change(
    request: magi_proto::Environment,
    session: &Arc<Mutex<Session>>,
    worker: &RwLock<Option<Arc<Worker>>>,
    models: &Arc<Models>,
    person: &Person,
    scribe: &crate::scribe::Held,
) -> (String, bool) {
    match apply(request, session, worker, models, person, scribe).await {
        Ok(message) => (message, true),
        Err(message) => (message, false),
    }
}

async fn apply(
    request: magi_proto::Environment,
    session: &Arc<Mutex<Session>>,
    worker: &RwLock<Option<Arc<Worker>>>,
    models: &Arc<Models>,
    person: &Person,
    scribe: &crate::scribe::Held,
) -> Result<String, String> {
    let mut boundary = worker.write().await;
    if session.lock().await.busy() {
        return Err("session is busy; set variables after queued work finishes".into());
    }
    let _fetching = models.fetching.lock().await;
    let mut catalog = models.catalog.write().await;
    let mut current: std::collections::BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    current.extend(catalog.environ.clone());
    let (updates, skipped) = if request.source {
        let path = parsing::path(&request.input, &current)?;
        let path = catalog.cwd.join(path);
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .open(path)
            .await
            .map_err(|_| "could not open export file".to_owned())?;
        let metadata = file
            .metadata()
            .await
            .map_err(|_| "could not inspect export file".to_owned())?;
        if !metadata.is_file() || metadata.len() > parsing::LIMIT as u64 {
            return Err("export file must be a regular file no larger than 256 KiB".into());
        }
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        file.take(parsing::LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "could not read export file".to_owned())?;
        let text = String::from_utf8(bytes).map_err(|_| "export file must be UTF-8".to_owned())?;
        parsing::exports(&text, &current, false)?
    } else {
        parsing::exports(&format!("export {}", request.input), &current, true)?
    };
    if updates.is_empty() {
        return Ok(format!(
            "No standalone supported exports found; skipped {skipped} unsupported export statements. No code executed."
        ));
    }
    let names = updates.keys().cloned().collect::<Vec<_>>().join(", ");
    catalog.environ.extend(updates);
    current.extend(catalog.environ.clone());
    for card in &mut catalog.cards {
        if let Some(key) = &card.needs {
            card.ready = current.get(key).is_some_and(|v| !v.is_empty());
        }
    }
    let existing = boundary.as_ref().and_then(|held| held.backend());
    let starting = existing.is_none();
    let backend = existing.or_else(|| catalog.chosen().and_then(|name| catalog.backend(&name)));
    let mut planning_for = None;
    if let Some(mut backend) = backend {
        backend.environ.clone_from(&catalog.environ);
        if starting {
            planning_for = backend
                .context_window
                .map(|window| (backend.model.clone(), window));
            let mut held = session.lock().await;
            held.set_model(Some(magi_proto::ModelInfo {
                name: backend.model.clone(),
                context_window: backend.context_window.unwrap_or(0),
            }));
            held.announce_model();
        }
        *boundary = Some(Arc::new(Worker::gated(
            backend,
            Some(Arc::clone(&person.approver)),
            Arc::clone(&person.asks),
            Arc::clone(&person.holds),
            Arc::clone(&person.knows),
            Arc::clone(scribe),
        )));
    }
    let choices = catalog.choices();
    let mut held = session.lock().await;
    held.set_choices(choices.clone());
    let _ = held
        .publisher()
        .send(magi_proto::HarnessEvent::ModelsRefreshed {
            choices,
            warning: None,
        });
    drop(held);
    if let Some((model, window)) = planning_for {
        let _ = tokio::time::timeout(crate::GREETING, async {
            let mut open = scribe.lock().await;
            if let Some(open) = open.as_mut() {
                let _ = open.note_model(&model, window).await;
            }
        })
        .await;
    }
    Ok(format!(
        "Session variables set: {names}. Skipped {skipped} unsupported export statements. No code executed; values are not saved."
    ))
}
