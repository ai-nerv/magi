//! Handing what a session settled to the memory layer, on a clock of its own.

use super::{Scribe, shape::beside};
use magi_ipc::family::Fault;
use magi_proto::{Entry, HarnessEvent};

/// The longest one flush may take, however many entries it carries and however slow the peer.
pub const FLUSH_PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);

/// Snapshot settled entries under the session lock, then write without holding it, acknowledging
/// each unchanged version only after its write succeeds. One entry the memory layer refuses is
/// left out and the rest still go; a memory layer that is not answering stops the flush, and the
/// queue it leaves is kept within bounds.
///
/// # Errors
/// Whatever balthasar answered. [`Fault::is_fatal`] says whether continuing would build on a hole.
pub async fn flush(
    session: &tokio::sync::Mutex<crate::session::Session>,
    scribe: &mut Option<Scribe>,
) -> Result<(), Fault> {
    let Some(scribe) = scribe.as_mut() else {
        // No memory layer at all, so nothing queued will ever be recorded.
        drop(session.lock().await.take_pending());
        return Ok(());
    };
    let handed = tokio::time::timeout(FLUSH_PATIENCE, handing(session, scribe))
        .await
        .unwrap_or_else(|_| {
            Err(Fault::Unavailable(format!(
                "the transcript was not handed over within {FLUSH_PATIENCE:?}"
            )))
        });
    if handed.as_ref().is_err_and(Fault::is_fatal) {
        let mut held = session.lock().await;
        if let Some(text) = held.bound_pending() {
            let _ = held.publisher().send(HarnessEvent::Noticed {
                cursor: magi_proto::Cursor::ZERO,
                text,
            });
        }
    }
    handed
}

async fn handing(
    session: &tokio::sync::Mutex<crate::session::Session>,
    scribe: &mut Scribe,
) -> Result<(), Fault> {
    scribe.recover_writer().await?;
    let settled: Vec<_> = {
        let held = session.lock().await;
        if !held.has_pending() {
            return Ok(());
        }
        held.pending_batch()
            .into_iter()
            .map(|(cursor, entry)| {
                let with = beside(&held, cursor, &entry);
                (cursor, entry, with)
            })
            .collect()
    };
    magi_model::noted!("scribe: handing {} entries to balthasar", settled.len());
    let mut refused: Option<(usize, Fault)> = None;
    for (cursor, entry, beside) in settled {
        // A mask is not news to the layer that ordered it: balthasar marks a turn masked as it
        // hands the plan over, and streaming it back would file its decision as a fresh turn.
        if !matches!(entry, Entry::Masked { .. }) {
            match scribe.settle(cursor, &entry, &beside).await {
                Ok(()) => {}
                Err(why) if why.is_fatal() => return Err(why),
                Err(why) => {
                    magi_model::noted!("scribe: entry {} was not taken: {why}", cursor.0);
                    let count = refused.as_ref().map_or(0, |(n, _)| *n);
                    refused = Some((count + 1, refused.map_or(why, |(_, first)| first)));
                }
            }
        }
        session.lock().await.acknowledge_pending(cursor, &entry);
    }
    match refused {
        None => Ok(()),
        Some((count, first)) => Err(Fault::Refused(format!(
            "{count} entr{} the memory layer would not take {} left out of the record: {first}",
            if count == 1 { "y" } else { "ies" },
            if count == 1 { "was" } else { "were" },
        ))),
    }
}

/// Tell whoever is attached what a flush came to: once when recording stops, once when it is
/// back, and each time an entry was left out of the record.
pub(crate) fn told(held: &mut crate::session::Session, flushed: &Result<(), Fault>) {
    let text = match flushed {
        Err(why) if !why.is_fatal() => {
            Some(format!("Part of this conversation was not recorded: {why}"))
        }
        _ => said_about(&mut held.unrecorded, flushed.is_ok()).map(str::to_owned),
    };
    if let Some(text) = text {
        let _ = held.publisher().send(HarnessEvent::Noticed {
            cursor: magi_proto::Cursor::ZERO,
            text,
        });
    }
}

/// What to tell the person when the memory layer stops answering, and when it answers again.
/// `told` is whether they already know: it is said once each way, not on every turn.
pub(crate) fn said_about(told: &mut bool, handed_over: bool) -> Option<&'static str> {
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
