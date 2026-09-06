//! Handing the prompt to `$EDITOR`, and the sizes the screen reports.
//!
//! Split out under THE RULE; the loop next door is what these serve.

use crate::app::App;
use crate::terminal::Session;
use anyhow::Result;

/// Hand the prompt to `$EDITOR`, releasing the terminal for the duration.
///
/// The raw-mode session is dropped first and rebuilt after: a full-screen editor and a TUI
/// cannot share a tty, and leaving raw mode on would hand the editor unreadable input.
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

/// Append a line to `$MAGI_DEBUG_LOG`, if it is set.
///
/// A UI owns the terminal, so `eprintln!` is not available for diagnosis — it would land in
/// the middle of the frame. This is the only way to see what the loop actually did.
///
/// Forwards to [`mod@magi_model::noted`], which is the same mechanism reading the same variable.
/// It started here and moved down to the leaf crate when the six process crossings needed it
/// too; keeping the name means the call sites in `driver.rs` read as they always did.
pub(super) fn debug_log(args: std::fmt::Arguments<'_>) {
    magi_model::noted::note(args);
}

/// How wide a tool's rows actually are, in columns.
///
/// **The box's inside, not the terminal's width.** A surface is drawn in the same slot a picker
/// and a completion are — inside the prompt box — so what it has is the width the box has, less
/// the bar and the padding down its left. Told the terminal's width instead, a tenant lays itself
/// out two columns wider than the space it is given and the right of every row is clipped.
pub(super) fn inner() -> u16 {
    crossterm::terminal::size()
        .map_or(80, |(cols, _)| cols)
        .saturating_sub(magi_tui::metric::gutter() + 1)
        .max(20)
}
