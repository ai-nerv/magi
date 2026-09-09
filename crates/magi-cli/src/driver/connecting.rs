//! Keeping a socket to the session, and redialling one that dropped.
//!
//! The other half of the driver: one loop reads the terminal and draws, this one owns the
//! connection. Split because they answer to different things — a keypress and a socket that went
//! away — and the only state they share is the two channels between them.
//!
//! **Which session is not fixed for the life of the loop.** The screen can be pointed at another
//! agent, so the target arrives on a watch rather than as an argument: the loop detaches, drops
//! the connection and dials the new one. A watch and not a channel because only the latest value
//! means anything — somebody who pressed the arrow four times wants the fourth agent, not four
//! connections in turn.

use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, UiCommand};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, watch};

use super::RECONNECT_DELAY;
use super::editing::{debug_log, inner};

/// Keep a connection to whichever session the screen is pointed at, redialling when it drops.
///
/// A dead session is not an error for the UI: it is the detach case, and reattaching with the
/// last cursor is how an in-flight turn is rejoined rather than replayed.
///
/// `own` is this process's own socket, and the only thing that separates the two cases. It is
/// handed in rather than inferred because everything else about a peer's socket and ours looks
/// identical from here — both are paths melchior or this process wrote.
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
        // **Only ever our own.** A `Drawing` is a count of the screens a session has, and two UIs
        // holding one on the same session is a tool given rows in a terminal that is not the one
        // its output will land in. A screen reading a peer is not a screen that session may use.
        let draws = socket == own;
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
            //
            // A peer's socket is the case that will not come back on its own — the agent has
            // gone — so the wait ends early when somebody points the screen somewhere else.
            tokio::select! {
                () = tokio::time::sleep(RECONNECT_DELAY) => {}
                // An error is the driver having gone, and a watch nobody holds answers at once
                // — so this arm must end the loop rather than take it round again.
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
        // Straight after the attach, and again on every resize. The session has no terminal, so a
        // tool given rows in this one has no other way to know how wide they are — and this is
        // sent on reconnect too, because the window may have changed while nothing was attached.
        //
        // The width only. How much room there is comes from the draw, which is the one place that
        // knows what the prompt and the footer have already taken; a number invented here would be
        // a grant made against a layout nobody had measured.
        //
        // Not to a peer. A screen that may not draw has no rows to grant, and a second opinion
        // about the width would reach a tool laying itself out for the window it is actually in.
        if draws {
            let _ = writer
                .write(&UiCommand::Sized {
                    rows: None,
                    cols: inner(),
                    holds: crate::terminal::reports_holds(),
                })
                .await;
        }

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

        // Whether this connection ended because the screen moved, which decides what the next one
        // asks for. See the reset below.
        let mut moved = false;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { return };
                    // **The last gate, and the one that closes a race the first cannot.** The UI
                    // refuses to *queue* anything that drives a session it is only reading, but a
                    // command queued a moment before the screen moved would be written to
                    // whoever it moved to. Dropped here rather than held: it was meant for a
                    // session this connection is no longer talking to.
                    if !draws && crate::app::drives(&command) {
                        debug_log(format_args!("dropped a command meant for our own session"));
                        continue;
                    }
                    // Awaited in the branch body, not as a select arm: a cancelled write
                    // desyncs the stream the same way a cancelled read does.
                    if writer.write(&command).await.is_err() {
                        break;
                    }
                }
                pointed = target.changed() => {
                    // Said rather than dropped. The session counts its screens, and one that
                    // learns a UI has gone by the socket closing has already granted rows to a
                    // tool nobody was watching.
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
        // **Nought for somebody new, and only for somebody new.** The cursor is a count of what
        // this screen already holds, and a session answers an attach by keeping back that many
        // entries. Carried across a swap it is a peer's whole history silently withheld — the
        // agent looks like it has never said anything. Carried across a *reconnect* it is what
        // rejoins an in-flight turn rather than replaying it, which is why the two cases differ.
        from_cursor = if moved {
            Cursor::ZERO
        } else {
            Cursor(cursor.load(Ordering::Relaxed))
        };

        // Not after a swap. The delay is there so a session that is still binding its socket is
        // not hammered; somebody who pressed an arrow is waiting on the screen in front of them.
        if !moved {
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }
}

#[cfg(test)]
#[path = "connecting/swapping.rs"]
mod swapping;
