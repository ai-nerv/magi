//! Starting a turn for something addressed to this session, and what happens after it.
//!
//! Split out under THE RULE; the session next door is what these drive. They belong together
//! because they are one path with a boundary in the middle: [`submit`] journals what opened a
//! turn and runs it, and [`after`] is everything the session owes once it has stopped —
//! flushing to balthasar, and answering whatever arrived while it was busy.

use super::*;

/// Journal what opened a turn, and run it.
///
/// `opening` is what was said and by whom: a prompt somebody typed, or a message another
/// instance sent. Both start a turn the same way and for the same reason — something addressed
/// to this session arrived and wants an answer — so they are one path rather than two that
/// would drift.
///
/// It is journalled before the provider is called, so an interrupted turn still shows what was
/// asked. Without a backend the refusal is a well-formed assistant entry rather than an error
/// out of band — the transcript stays uniform and the UI needs no second path.
pub(super) async fn submit(
    session: &Arc<Mutex<Session>>,
    opening: Entry,
    worker: Option<Arc<worker::Worker>>,
    catalog: &crate::catalog::Catalog,
    scribe: &Arc<Mutex<Option<crate::scribe::Scribe>>>,
) -> Result<(), HostError> {
    {
        let mut held = session.lock().await;
        // A stop belongs to the turn it interrupted. Left set, it would cancel the prompt typed
        // to replace the one the user just stopped.
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

    // Spawned, not awaited. This runs on the connection's own task, which is also the task
    // forwarding events to the attached UI: waiting here means nothing reaches the screen until
    // the turn is over, so a streaming response arrives all at once at the end.
    //
    // Overlapping turns are not a risk. The worker is one thread taking one job at a time, so
    // a second prompt queues behind the first exactly as it did when this awaited.
    let session = Arc::clone(session);
    let scribe = Arc::clone(scribe);
    tokio::spawn(async move { after(session, worker, scribe).await });
    Ok(())
}

/// Run a turn, then deal with whatever arrived while it was running.
///
/// The other half of the waiting room. An arrival during a turn is held rather than delivered
/// — see [`session::Session::waiting`] — and this is where it comes back out: the turn ends,
/// the messages are committed in the order they came, and if any of them wanted an answer, one
/// more turn runs to give it.
///
/// A loop, because more can arrive during *that* turn. It ends when a turn finishes with an
/// empty waiting room, which is the ordinary case: a session with nobody talking to it does one
/// pass and stops.
async fn after(
    session: Arc<Mutex<Session>>,
    worker: Arc<worker::Worker>,
    scribe: Arc<Mutex<Option<crate::scribe::Scribe>>>,
) {
    loop {
        worker.run(Arc::clone(&session)).await;

        // The turn boundary, which is where durability is owed. Amendments during streaming are
        // coalesced by cursor in the session, so a message written a hundred times on the way
        // through goes over once, as it finally stood.
        if let Err(fault) = crate::scribe::flush(&session, &mut *scribe.lock().await).await {
            // Said once, in the transcript, rather than swallowed. A session whose transcript
            // stopped being recorded must not look like one that is fine, and while magi's own
            // journal is still the copy of record this costs memory rather than the session.
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
        // Committed together and answered once. Ten subagents reporting during one turn is ten
        // things to read and one turn to read them in — waking once per message would spend a
        // turn on each and let the last of them arrive during the answer to the first.
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
