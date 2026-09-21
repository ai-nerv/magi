use crate::{scribe, session::Session, worker::Worker};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

#[cfg(test)]
mod tests;

pub(super) async fn resume(
    session: &Arc<Mutex<Session>>,
    worker: &RwLock<Option<Arc<Worker>>>,
    scribe: &scribe::Held,
    id: &str,
) -> Result<(), String> {
    let _boundary = worker.write().await;
    let (transition, helpers) = {
        let held = session.lock().await;
        (held.begin_resume()?, held.helpers().pause()?)
    };
    scribe::flush(session, &mut *scribe.lock().await)
        .await
        .map_err(|why| format!("cannot leave this session: {why}"))?;
    helpers.drain(std::time::Duration::from_secs(30)).await?;
    let mut open = scribe.lock().await;
    scribe::flush(session, &mut open)
        .await
        .map_err(|why| format!("cannot leave this session: {why}"))?;
    let (mut next, journal) = open
        .as_ref()
        .ok_or("no memory layer is connected")?
        .prepare_resume(id)
        .await
        .map_err(|why| format!("cannot resume {id:?}: {why}"))?;
    let model = session.lock().await.model();
    if let Some(model) = model {
        match next.note_model(&model.name, model.context_window).await {
            Ok(()) | Err(magi_ipc::family::Fault::Refused(_)) => {}
            Err(why) => return Err(format!("cannot prepare resumed model: {why}")),
        }
    }
    let mut held = session.lock().await;
    *open = Some(next);
    helpers.retire();
    drop(transition);
    held.resume_prepared(journal);
    Ok(())
}
