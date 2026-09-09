//! Handing the prompt to `$EDITOR`, and the sizes the screen reports.

use crate::app::App;
use crate::terminal::Session;
use anyhow::Result;

/// Hand the prompt to `$EDITOR`, releasing the terminal for the duration: the raw-mode session is
/// dropped first and rebuilt after, since a full-screen editor and a TUI cannot share a tty.
pub(super) fn external_edit(session: &mut Session, app: &mut App) -> Result<()> {
    let before = app.editor.text();
    let Some(editor) = crate::external_editor::editor_command() else {
        app.show_notice("no $EDITOR or $VISUAL is set".into());
        return Ok(());
    };

    let placeholder = Session::open()?;
    let previous = std::mem::replace(session, placeholder);
    drop(previous);

    let edited = crate::external_editor::edit_with(&editor, &before);

    *session = Session::open()?;
    session.terminal.clear()?;

    match edited {
        Ok(Some(text)) => app.editor.set_text(&text),
        Ok(None) => {}
        Err(e) => app.show_notice(format!("editor failed: {e}")),
    }
    Ok(())
}

/// Append a line to `$MAGI_DEBUG_LOG`, if it is set. Forwards to [`mod@magi_model::noted`].
pub(super) fn debug_log(args: std::fmt::Arguments<'_>) {
    magi_model::noted::note(args);
}

/// How wide a tool's rows actually are: the prompt box's inside, not the terminal's width, so a
/// tenant given the terminal width lays itself out wider than its slot and is clipped.
pub(super) fn inner() -> u16 {
    crossterm::terminal::size()
        .map_or(80, |(cols, _)| cols)
        .saturating_sub(magi_tui::metric::gutter() + 1)
        .max(20)
}
