//! Which agent the screen is pointed at: moving between them, and what may be sent to one.
//!
//! Split out under THE RULE; the loop this serves is next door. The decisions themselves live in
//! [`crate::app::crewing`] — this is the wiring between them and the two channels the driver
//! owns: the watch the connection loop is dialling from, and the commands going the other way.

use crate::app::App;
use magi_proto::UiCommand;
use magi_tui::footer::FooterData;
use tokio::sync::{mpsc, watch};

/// Send a command, unless it would drive an agent this screen is only reading.
///
/// **The one funnel.** Every write the UI performs goes through here, so a command added later is
/// gated by having been sent at all rather than by somebody remembering to guard it — which is
/// what "the write side is gated in the UI" has to mean if it is to survive the next feature.
pub(super) async fn direct(app: &mut App, to: &mpsc::Sender<UiCommand>, command: UiCommand) {
    if app.attached.is_some() && crate::app::drives(&command) {
        if crate::app::spoken(&command) {
            app.refuse_drive();
        }
        return;
    }
    let _ = to.send(command).await;
}

/// Point the screen at the next agent along, and tell the connection loop where to dial.
///
/// `own` is this session's own socket, which the app does not hold: it is named from a key made
/// before melchior was asked anything, and a second copy of that name would be a second opinion
/// about a file.
///
/// `held` is what arrived for *this* session while the screen was elsewhere — see [`ours`]. It is
/// let go the moment we are back, because those commands were always ours.
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
    // Sent even when it names the socket already dialled, which happens when the ring is walked
    // right round: `watch::Sender::send` wakes the loop either way, and the loop's own reset is
    // what makes a re-dial of our own session the fresh attach the cleared screen now needs.
    let _ = target.send(at);
    if app.attached.is_none() {
        for command in held.drain(..) {
            let _ = to.send(command).await;
        }
    }
    true
}

/// Keep a command for this session until the screen is back on it.
///
/// What melchior hands the harness — an arrival to put in the transcript, permissions a parent
/// lent us — belongs to *this* session, and would land on a peer if it were sent while the screen
/// is elsewhere. Dropping it would lose a sibling's message from the only place it was going to
/// be read; the layer's own inbox still has it, and nothing here would say why it never appeared.
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

/// The footer as of now.
///
/// Rebuilt each frame from what the session has reported rather than kept in step by hand: the
/// numbers change on every delta, and a copy updated at each of the places that could change
/// them is a copy that misses one.
pub(super) fn footer_data(app: &App) -> FooterData {
    let window = app.model.as_ref().map_or(0, |m| m.context_window);
    FooterData {
        // Whoever is on screen, which is not always this session. A peer is `role/id`: the
        // project cannot differ, and this row loses whole columns to make room.
        identity: app.viewing(),
        // **What the arrows can actually reach**, not what melchior can name. Its roster is every
        // session listening in the project — this one included, since it dials its own socket like
        // any other — and an agent that published no screen is a name with nowhere to look.
        crew: app.crew_size(),
        own: app.attached.is_none(),
        model: app.model.as_ref().map_or_else(
            || magi_tui::glyph::no_model().to_owned(),
            |model| model.name.clone(),
        ),
        input_tokens: app.usage().prompt_tokens(),
        output_tokens: app.usage().output,
        context_window: window,
        // Against the last turn's prompt, not the running total: the window holds one
        // conversation, and a session that has spent ten windows over an afternoon is not
        // ten times full. `None` until a model says how big its window is, which is what the
        // footer's question mark means.
        context_percent: (window > 0).then(|| {
            let used = app.last_prompt_tokens();
            (used as f64 / window as f64) * 100.0
        }),
    }
}

#[cfg(test)]
#[path = "crewing/tests.rs"]
mod tests;
