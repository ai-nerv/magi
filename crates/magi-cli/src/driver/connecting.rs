//! Keeping a socket to the session, and redialling one that dropped.
//!
//! The other half of the driver: one loop reads the terminal and draws, this one owns the
//! connection. Split because they answer to different things — a keypress and a socket that went
//! away — and the only state they share is the two channels between them.

use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

use super::RECONNECT_DELAY;
use super::editing::{debug_log, inner};

/// Keep a connection to the session, redialling when it drops.
///
/// A dead session is not an error for the UI: it is the detach case, and reattaching with the
/// last cursor is how an in-flight turn is rejoined rather than replayed.
pub(super) async fn connection_loop(
    socket: std::path::PathBuf,
    events: mpsc::Sender<HarnessEvent>,
    mut commands: mpsc::Receiver<UiCommand>,
    mut from_cursor: Cursor,
    attached: Arc<std::sync::atomic::AtomicBool>,
) {
    loop {
        attached.store(false, Ordering::Relaxed);
        let Ok(stream) = magi_ipc::connect(&socket).await else {
            debug_log(format_args!("connect failed"));
            // Waited out rather than restarted. There is nothing to restart: the session is a
            // task in this process, so a socket that will not answer means this process is
            // still binding it — the only race left — or has begun shutting it down, and
            // either way the loop ends when the process does.
            //
            // This used to spawn a session. It had to: the session was a separate process that
            // could crash, be killed, or be lost to a sleeping machine, and a UI with nothing
            // to talk to had to build itself a new one and resume the journal. None of those
            // can happen to something that dies exactly when its window does.
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        };

        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);

        if writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor,
                // There is a terminal on this end, so a tool may be given rows in it.
                draws: true,
            })
            .await
            .is_err()
        {
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }
        // Straight after the attach, and again on every resize. The session has no terminal, so a
        // tool given rows in this one has no other way to know how wide they are — and this is
        // sent on reconnect too, because the window may have changed while nothing was attached.
        //
        // The width only. How much room there is comes from the draw, which is the one place that
        // knows what the prompt and the footer have already taken; a number invented here would be
        // a grant made against a layout nobody had measured.
        let _ = writer
            .write(&UiCommand::Sized {
                rows: None,
                cols: inner(),
                holds: crate::terminal::reports_holds(),
            })
            .await;

        // Reads run in their own task because `FrameReader::read` is not cancel-safe: it takes
        // a length and then a body, and a `select!` that drops it between the two leaves the
        // next read parsing body bytes as a length. Sending a command used to do exactly that,
        // which desynced the stream on the first prompt.
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

        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { return };
                    // Awaited in the branch body, not as a select arm: a cancelled write
                    // desyncs the stream the same way a cancelled read does.
                    if writer.write(&command).await.is_err() {
                        break;
                    }
                }
                _ = &mut reading => break,
            }
        }

        reading.abort();
        from_cursor = Cursor(cursor.load(Ordering::Relaxed));

        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}
