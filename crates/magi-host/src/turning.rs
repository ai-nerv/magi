//! Starting a turn for something addressed to this session, and what happens after it. [`submit`]
//! journals what opened a turn and runs it; [`after`] flushes to balthasar and answers arrivals.

use super::*;

/// Journal what opened a turn, and run it. Journalled before the provider is called, so an
/// interrupted turn still shows what was asked; without a backend the refusal is a well-formed
/// assistant entry rather than an error out of band.
pub(super) async fn submit(
    session: &Arc<Mutex<Session>>,
    opening: Entry,
    worker: Option<Arc<worker::Worker>>,
    catalog: &crate::catalog::Catalog,
    scribe: &Arc<Mutex<Option<crate::scribe::Scribe>>>,
) -> Result<(), HostError> {
    {
        let mut held = session.lock().await;
        // A stop belongs to the turn it interrupted, or it cancels the replacement prompt.
        held.cancel().clear();
        held.commit(opening)?;
    }

    let Some(worker) = worker else {
        let mut held = session.lock().await;
        let id = MessageId::new(format!("a{}", held.cursor().next().0));
        held.commit(Entry::Assistant {
            id,
            text: String::new(),
            thinking: String::new(),
            stop_reason: Some(StopReason::Error),
            error: Some(no_model(catalog)),
            signatures: magi_proto::Signatures::default(),
            usage: magi_proto::Usage::default(),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(());
    };

    // Spawned, not awaited: this task also forwards events to the attached UI, so awaiting would
    // hold the whole streaming response back until the turn ended. The worker takes one job at a time.
    let session = Arc::clone(session);
    let scribe = Arc::clone(scribe);
    tokio::spawn(async move { after(session, worker, scribe).await });
    Ok(())
}

/// Run a turn, then deal with whatever arrived while it was running. An arrival during a turn is
/// held — see [`session::Session::waiting`] — and comes back out here. Loops, because more can
/// arrive during that turn.
async fn after(
    session: Arc<Mutex<Session>>,
    worker: Arc<worker::Worker>,
    scribe: Arc<Mutex<Option<crate::scribe::Scribe>>>,
) {
    loop {
        worker.run(Arc::clone(&session)).await;

        // The turn boundary, which is where durability is owed.
        if let Err(fault) = crate::scribe::flush(&session, &mut *scribe.lock().await).await {
            let mut held = session.lock().await;
            let id = MessageId::new(format!("n{}", held.cursor().next().0));
            let _ = held.commit(Entry::Assistant {
                id,
                text: String::new(),
                thinking: String::new(),
                stop_reason: Some(StopReason::Error),
                error: Some(format!("this turn was not recorded: {fault}")),
                signatures: magi_proto::Signatures::default(),
                usage: magi_proto::Usage::default(),
            });
        }

        let arrived = session.lock().await.release();
        if arrived.is_empty() {
            return;
        }
        // Committed together and answered once: waking per message would spend a turn on each.
        let mut answer = false;
        {
            let mut held = session.lock().await;
            for entry in arrived {
                answer |= wants_answering(&entry);
                if held.commit(entry).is_err() {
                    return;
                }
            }
        }
        if !answer {
            return;
        }
    }
}
