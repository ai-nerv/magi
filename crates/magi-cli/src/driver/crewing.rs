//! Which agent the screen is pointed at: moving between them, and what may be sent to one. The
//! decisions live in `app::crewing`; this is the wiring to the driver's two channels.

use crate::app::App;
use magi_proto::UiCommand;
use magi_tui::footer::FooterData;
use tokio::sync::{mpsc, watch};

/// Send a command, unless it would drive an agent this screen is only reading. Every write the UI
/// performs goes through here, so a command added later is gated by having been sent at all.
pub(super) async fn direct(app: &mut App, to: &mpsc::Sender<UiCommand>, command: UiCommand) {
    if app.attached.is_some() && crate::app::drives(&command) {
        if crate::app::spoken(&command) {
            app.refuse_drive();
        }
        return;
    }
    let _ = to.send(command).await;
}

/// Point the screen at the next agent along, and tell the connection loop where to dial. `own` is
/// this session's own socket, which the app does not hold; `held` is what arrived for *this* session
/// while the screen was elsewhere (see [`ours`]), and is let go the moment we are back.
pub(super) async fn walk(
    app: &mut App,
    forward: bool,
    own: &std::path::Path,
    target: &watch::Sender<std::path::PathBuf>,
    to: &mpsc::Sender<UiCommand>,
    held: &mut Vec<UiCommand>,
) -> bool {
    let Some(seat) = app.step(forward) else {
        return false;
    };
    let at = match seat {
        crate::app::Seat::Own => own.to_path_buf(),
        crate::app::Seat::Peer(at) => at,
    };
    // Sent even when it names the socket already dialled: `watch::Sender::send` wakes the loop
    // either way, and the loop's own reset makes a re-dial of our own session a fresh attach.
    let _ = target.send(at);
    if app.attached.is_none() {
        for command in held.drain(..) {
            let _ = to.send(command).await;
        }
    }
    true
}

/// Keep a command for this session until the screen is back on it. Sent while the screen is
/// elsewhere it would land on a peer; dropped it would lose a sibling's message unread.
pub(super) async fn ours(
    app: &App,
    to: &mpsc::Sender<UiCommand>,
    held: &mut Vec<UiCommand>,
    command: UiCommand,
) {
    if app.attached.is_some() {
        held.push(command);
        return;
    }
    let _ = to.send(command).await;
}

pub(super) fn footer_data(app: &App) -> FooterData {
    let window = app.model.as_ref().map_or(0, |m| m.context_window);
    FooterData {
        identity: app.viewing(),
        // What the arrows can reach, not what melchior can name.
        crew: app.crew_size(),
        own: app.attached.is_none(),
        model: app.model.as_ref().map_or_else(
            || magi_tui::glyph::no_model().to_owned(),
            |model| model.name.clone(),
        ),
        input_tokens: app.usage().prompt_tokens(),
        output_tokens: app.usage().output,
        context_window: window,
        // Against the last turn's prompt, not the running total.
        context_percent: (window > 0).then(|| {
            let used = app.last_prompt_tokens();
            (used as f64 / window as f64) * 100.0
        }),
    }
}

#[cfg(test)]
#[path = "crewing/tests.rs"]
mod tests;
