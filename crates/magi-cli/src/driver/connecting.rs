//! Keeping a socket to the session, and redialling one that dropped. Which session is not fixed, so
//! the target arrives on a watch — only the latest value means anything.

use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, watch};

use super::RECONNECT_DELAY;
use super::editing::{debug_log, inner};

/// Keep a connection to whichever session the screen is pointed at, redialling when it drops. A dead
/// session is the detach case, not an error; `own` is what separates a peer's socket from ours.
pub(super) async fn connection_loop(
    own: std::path::PathBuf,
    mut target: watch::Receiver<std::path::PathBuf>,
    events: mpsc::Sender<HarnessEvent>,
    mut commands: mpsc::Receiver<UiCommand>,
    mut from_cursor: Cursor,
    attached: Arc<std::sync::atomic::AtomicBool>,
) {
    loop {
        attached.store(false, Ordering::Relaxed);
        let socket = target.borrow_and_update().clone();
        // Only ever our own: two UIs drawing one session is a tool given rows in the wrong terminal.
        let draws = socket == own;
        let Ok(stream) = magi_ipc::connect(&socket).await else {
            debug_log(format_args!("connect failed"));
            // Waited out rather than restarted: the session is a task in this process. A peer's
            // socket never comes back, so the wait ends early when the screen moves.
            tokio::select! {
                () = tokio::time::sleep(RECONNECT_DELAY) => {}
                // A watch nobody holds answers at once, so this arm must end the loop.
                pointed = target.changed() => {
                    if pointed.is_err() { return }
                    from_cursor = Cursor::ZERO;
                }
            }
            continue;
        };

        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);

        if writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor,
                draws,
            })
            .await
            .is_err()
        {
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }
        // Straight after the attach and on every resize, including reconnects. The width only: how
        // much room there is comes from the draw. Not to a peer, which has no rows to grant.
        if draws {
            let _ = writer
                .write(&UiCommand::Sized {
                    rows: None,
                    cols: inner(),
                    holds: crate::terminal::reports_holds(),
                })
                .await;
        }

        // Reads run in their own task because `FrameReader::read` is not cancel-safe: it takes a
        // length and then a body, and a `select!` dropping it between the two leaves the next read
        // parsing body bytes as a length.
        attached.store(true, Ordering::Relaxed);
        let cursor = Arc::new(AtomicU64::new(from_cursor.0));
        let reader_cursor = Arc::clone(&cursor);
        let reader_events = events.clone();
        let mut reading = tokio::spawn(async move {
            loop {
                match reader.read::<HarnessEvent>().await {
                    Ok(event) => {
                        reader_cursor.fetch_max(event.cursor().0, Ordering::Relaxed);
                        if reader_events.send(event).await.is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });

        let mut moved = false;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { return };
                    // A command queued a moment before the screen moved would be written to whoever
                    // it moved to. Dropped rather than held.
                    if !draws && crate::app::drives(&command) {
                        debug_log(format_args!("dropped a command meant for our own session"));
                        continue;
                    }
                    // Awaited in the branch body, not as a select arm: a cancelled write desyncs.
                    if writer.write(&command).await.is_err() {
                        break;
                    }
                }
                pointed = target.changed() => {
                    // Said rather than dropped: the session counts its screens.
                    let _ = writer.write(&UiCommand::Detach).await;
                    if pointed.is_err() {
                        reading.abort();
                        return;
                    }
                    moved = true;
                    break;
                }
                _ = &mut reading => break,
            }
        }

        reading.abort();
        // Carried across a swap the cursor is a peer's whole history silently withheld; across a
        // reconnect it is what rejoins an in-flight turn rather than replaying it.
        from_cursor = if moved {
            Cursor::ZERO
        } else {
            Cursor(cursor.load(Ordering::Relaxed))
        };

        // Not after a swap. The delay keeps a session still binding its socket from being hammered.
        if !moved {
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }
}

#[cfg(test)]
#[path = "connecting/swapping.rs"]
mod swapping;
