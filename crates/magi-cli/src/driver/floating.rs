//! What the keys do on an open float that has tabs. The decisions live in `app::views`.

use super::crewing::direct;
use crate::app::App;
use magi_proto::UiCommand;
use tokio::sync::mpsc;

/// Step the heading strip, and ask for whatever the tab it landed on needs.
pub(super) async fn step(app: &mut App, to: &mpsc::Sender<UiCommand>, forward: bool) {
    if let Some(command) = app.step_memory(forward) {
        direct(app, to, command).await;
    }
}

/// Enter on a memory: ask what it rests on, and step to the tab that says so.
pub(super) async fn weigh(app: &mut App, to: &mpsc::Sender<UiCommand>, id: &str) {
    if let Some(command) = app.weigh(id) {
        direct(app, to, command).await;
    }
    app.show_worth();
}

/// ←/→ on a card whose cursor is on a setting: the model's card, and the permission card.
pub(super) async fn fold(app: &mut App, to: &mpsc::Sender<UiCommand>, open: bool) {
    let stepped = if app.pane_titled("model") {
        app.adjust_model(open)
    } else {
        app.adjust_permission(open)
    };
    if let Some(command) = stepped {
        direct(app, to, command).await;
    }
}
