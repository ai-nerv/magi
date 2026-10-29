//! Session-wide admission and turn-boundary persistence.

use super::*;
use crate::session::admission::Request;

pub(super) async fn submit(
    session: &Arc<Mutex<Session>>,
    request: Request,
    workers: &tokio::sync::RwLock<Option<Arc<worker::Worker>>>,
    catalog: &crate::catalog::Catalog,
    scribe: &crate::scribe::Held,
    cancellation: &[crate::cancel::Cancel],
    trace: &mut crate::session::observing::Trace,
) -> Result<(), String> {
    let worker = workers.read().await;
    if let Some(entry @ Entry::Workflow { .. }) = request.entry() {
        let open = scribe.lock().await;
        open.as_ref()
            .ok_or("workflow prompts require a configured history store")?
            .validate_workflow(entry)
            .map_err(|why| why.to_string())?;
    }
    let admitted = {
        let mut held = session.lock().await;
        if cancellation.iter().any(crate::cancel::Cancel::is_requested) {
            trace.finish(crate::events::TurnOutcome::Cancelled);
            return Err("prompt preparation was cancelled before admission".into());
        }
        held.admit_observed(request, std::mem::take(trace))?
    };
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
    let mut unread = Vec::new();
    let mut answer = answers(&mut held, &entry, &mut unread);
    if let Entry::User { id, .. } | Entry::Workflow { id, .. } = &mut entry {
        *id = MessageId::new(format!("u{}", held.cursor().next().0));
    }
    let arrivals = if matches!(entry, Entry::From { .. }) {
        held.take_arrivals()
    } else {
        Vec::new()
    };
    let committed = held.commit(entry).map_err(|why| why.to_string());
    // Every arrival already taken off the queue is committed, whatever became of the one before.
    for arrived in arrivals {
        answer |= answers(&mut held, &arrived, &mut unread);
        if let Err(why) = held.commit(arrived) {
            unread.push(why.to_string());
        }
    }
    for why in unread {
        let _ = held.publisher().send(HarnessEvent::Noticed {
            cursor: magi_proto::Cursor::ZERO,
            text: format!("A message that arrived could not be taken as sent: {why}"),
        });
    }
    committed?;
    Ok(answer)
}

/// Whether `entry` should start a turn. A report notification that cannot be read is said aloud
/// and is otherwise an ordinary message.
fn answers(held: &mut Session, entry: &Entry, unread: &mut Vec<String>) -> bool {
    let plain =
        matches!(entry, Entry::User { .. } | Entry::Workflow { .. }) || wants_answering(entry);
    match held.track_report(entry) {
        Ok(Some(report)) => report,
        Ok(None) => plain,
        Err(why) => {
            unread.push(why);
            plain
        }
    }
}

/// Run each admitted request in turn. Whatever one of them does, panic included, the session is
/// told the request is over, so it never stays busy on a turn that is gone.
async fn after(
    session: Arc<Mutex<Session>>,
    mut admitted: (u64, Request),
    worker: Option<Arc<worker::Worker>>,
    scribe: crate::scribe::Held,
    missing: String,
) {
    loop {
        let (owner, request) = admitted;
        let began = session.lock().await.cursor();
        let outcome = crate::catching::caught(handle(&session, request, worker.as_ref(), &missing))
            .await
            .unwrap_or_else(|why| Err(format!("this request stopped on an internal error: {why}")));
        let failed = outcome.is_err();
        if let Err(why) = outcome {
            error(&mut *session.lock().await, why);
        }
        let flushed = crate::catching::caught(async {
            crate::scribe::flush(&session, &mut *scribe.lock().await).await
        })
        .await
        .unwrap_or_else(|why| Err(magi_ipc::family::Fault::Failed(why)));
        let mut held = session.lock().await;
        crate::scribe::told(&mut held, &flushed);
        // A turn the memory layer did not take ends on an error rather than as a recorded one.
        if let Err(why) = &flushed
            && why.is_fatal()
            && answered_since(&held, began)
        {
            error(&mut held, format!("this turn was not recorded: {why}"));
        }
        held.observe_finished(owner, began, failed || flushed.is_err());
        let Some(next) = held.finish(owner) else {
            return;
        };
        admitted = next;
    }
}

/// Whether an answer that is not an error was committed after `began`.
fn answered_since(held: &Session, began: magi_proto::Cursor) -> bool {
    held.entries()
        .iter()
        .enumerate()
        .rev()
        .map_while(|(at, entry)| held.cursor_at(at).filter(|&c| c > began).map(|_| entry))
        .any(|entry| matches!(entry, Entry::Assistant { error: None, .. }))
}

/// One admitted request, run to its end.
async fn handle(
    session: &Arc<Mutex<Session>>,
    request: Request,
    worker: Option<&Arc<worker::Worker>>,
    missing: &str,
) -> Result<(), String> {
    let missing = || Err(missing.to_owned());
    match request {
        Request::Opening(entry) | Request::Tracked { entry, .. } => {
            match opening(session, entry).await? {
                true => match worker {
                    Some(worker) => worker.run(Arc::clone(session)).await,
                    None => missing(),
                },
                false => Ok(()),
            }
        }
        Request::Declare => match worker {
            Some(worker) => worker.declare(Arc::clone(session)).await,
            None => missing(),
        },
        Request::Grants(grants) => match worker {
            Some(worker) => worker.take_on(Arc::clone(session), grants).await,
            None => missing(),
        },
        Request::Reports if session.lock().await.pending_reports().is_empty() => Ok(()),
        Request::Reports => {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            let _ = session.lock().await.commit(Entry::From {
                who: "magi".into(), kin: "harness".into(), sort: "report_retry".into(),
                text: "Resume handling the pending subagent reports. The previous turn was interrupted; reports remain unacknowledged. Do not repeat completed changes or exceed the original task.".into(),
            });
            match worker {
                Some(worker) => worker.run(Arc::clone(session)).await,
                None => missing(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::admission::Request;

    fn report(who: &str, text: &str) -> Entry {
        Entry::From {
            who: who.into(),
            kin: "child".into(),
            sort: "report".into(),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn one_bad_report_does_not_drop_the_arrivals_queued_after_it() {
        let session = Arc::new(Mutex::new(Session::recorded(
            magi_proto::SessionId::new("s"),
            Vec::new(),
        )));
        let good = format!("{{\"revision\":\"{}\"}}", "a".repeat(64));
        {
            let mut held = session.lock().await;
            let _ = held.admit(Request::Declare).expect("reserved");
            for queued in [report("p/c/one", "not json"), report("p/c/two", &good)] {
                assert!(
                    held.admit(Request::Opening(queued))
                        .expect("queued")
                        .is_none()
                );
            }
        }
        let answer = opening(&session, report("p/c/zero", "also not json"))
            .await
            .expect("committed");
        assert!(answer, "the readable report still starts a turn");
        let held = session.lock().await;
        assert_eq!(
            held.entries().len(),
            3,
            "an arrival was dropped: {:?}",
            held.entries()
        );
        assert_eq!(held.pending_reports().len(), 1);
    }
}
