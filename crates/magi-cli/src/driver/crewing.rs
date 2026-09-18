//! Which agent the screen is pointed at: moving between them, and what may be sent to one. The
//! decisions live in `app::crewing`; this is the wiring to the driver's two channels.

use crate::app::App;
use magi_proto::UiCommand;
use magi_tui::footer::FooterData;
use tokio::sync::{mpsc, watch};

/// Send a command to whichever session is on screen: attaching is driving, and `--view-only` is
/// not. Only this terminal's geometry stays home, since that session draws nothing here.
pub(super) async fn direct(app: &mut App, to: &mpsc::Sender<UiCommand>, command: UiCommand) {
    if app.view_only && crate::app::changes(&command) {
        app.refuse_view_only();
        return;
    }
    if app.attached.is_some() && crate::app::for_screen(&command) {
        return;
    }
    // An answer that was really sent closes its question, and the next one still open takes the
    // screen. Only here, past the refusals above: a question a view-only screen could not answer
    // is still open.
    let settled = match &command {
        UiCommand::Permit { id, .. } | UiCommand::Answered { id, .. } => Some(id.clone()),
        _ => None,
    };
    let _ = to.send(command).await;
    if let Some(id) = settled {
        app.ask_settled(&id);
    }
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
    dial(app, seat, own, target, to, held).await;
    true
}

/// Tell the connection loop to dial `seat`, which the app already points at, and let go of what was
/// held for this session if the screen is back on it. Shared by the arrows, a click and the keys.
pub(super) async fn dial(
    app: &App,
    seat: crate::app::Seat,
    own: &std::path::Path,
    target: &watch::Sender<std::path::PathBuf>,
    to: &mpsc::Sender<UiCommand>,
    held: &mut Vec<UiCommand>,
) {
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
        // The count melchior reports; no longer drawn, but the roster still carries it.
        crew: app.crew_size(),
        own: app.attached.is_none(),
        name_hover: app.name_hover,
        model_hover: app.model_hover,
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

/// The line, if any, a watched agent's phase change is worth putting in front of a person. Only the
/// edges that end a wait — `finished`, `blocked` and `lost` — the rest is left to the panel.
pub(super) fn signal_notice(from: &str, kind: &str, cause: Option<&str>) -> Option<String> {
    match kind {
        "finished" => Some(format!("`{from}` finished.")),
        "lost" => Some(format!("`{from}` is gone without finishing.")),
        "blocked" => Some(match cause {
            Some(why) => format!("`{from}` is blocked: {why}"),
            None => format!("`{from}` is blocked."),
        }),
        _ => None,
    }
}

#[cfg(test)]
#[path = "crewing/tests.rs"]
mod tests;
