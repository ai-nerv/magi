//! Session-wide admission and turn-boundary persistence.

use super::*;
use crate::session::admission::Request;

pub(super) async fn submit(
    session: &Arc<Mutex<Session>>,
    request: Request,
    workers: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    scribe: &crate::scribe::Held,
) -> Result<(), String> {
    let worker = workers.read().await;
    let admitted = session.lock().await.admit(request)?;
    if let Some(admitted) = admitted {
        let session = Arc::clone(session);
        let worker = worker.clone();
        let scribe = Arc::clone(scribe);
        let missing = no_model(catalog);
        tokio::spawn(async move { after(session, admitted, worker, scribe, missing).await });
    }
    Ok(())
}

fn error(held: &mut Session, message: String) {
    let id = MessageId::new(format!("a{}", held.cursor().next().0));
    let _ = held.commit(Entry::Assistant {
        id,
        text: String::new(),
        thinking: String::new(),
        stop_reason: Some(StopReason::Error),
        error: Some(message),
        signatures: Default::default(),
        usage: Default::default(),
    });
}

async fn opening(session: &Arc<Mutex<Session>>, mut entry: Entry) -> Result<bool, String> {
    let mut held = session.lock().await;
    let mut answer = matches!(entry, Entry::User { .. }) || wants_answering(&entry);
    if let Entry::User { id, .. } = &mut entry {
        *id = MessageId::new(format!("u{}", held.cursor().next().0));
    }
    let arrivals = if matches!(entry, Entry::From { .. }) {
        held.take_arrivals()
    } else {
        Vec::new()
    };
    held.commit(entry).map_err(|why| why.to_string())?;
    for arrived in arrivals {
        answer |= wants_answering(&arrived);
        held.commit(arrived).map_err(|why| why.to_string())?;
    }
    Ok(answer)
}

async fn after(
    session: Arc<Mutex<Session>>,
    mut admitted: (u64, Request),
    worker: Option<Arc<worker::Worker>>,
    scribe: crate::scribe::Held,
    missing: String,
) {
    loop {
        let (owner, request) = admitted;
        let outcome = match request {
            Request::Opening(entry) => match opening(&session, entry).await {
                Ok(true) => match &worker {
                    Some(worker) => worker.run(Arc::clone(&session)).await,
                    None => Err(missing.clone()),
                },
                Ok(false) => Ok(()),
                Err(why) => Err(why),
            },
            Request::Declare => match &worker {
                Some(worker) => worker.declare(Arc::clone(&session)).await,
                None => Err(missing.clone()),
            },
            Request::Grants(grants) => match &worker {
                Some(worker) => worker.take_on(Arc::clone(&session), grants).await,
                None => Err(missing.clone()),
            },
        };
        if let Err(why) = outcome {
            error(&mut *session.lock().await, why);
        }
        if let Err(why) = crate::scribe::flush(&session, &mut *scribe.lock().await).await {
            error(
                &mut *session.lock().await,
                format!("this turn was not recorded: {why}"),
            );
        }
        let Some(next) = session.lock().await.finish(owner) else {
            return;
        };
        admitted = next;
    }
}
