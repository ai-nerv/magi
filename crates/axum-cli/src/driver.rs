//! The UI event loop.
//!
//! Three sources feed one `select!`: the socket, the terminal, and a spinner timer. State
//! lives in [`App`], drawing lives in [`ui`], and this file owns only the wiring — which is
//! what keeps the loop small enough to read.

use crate::app::{App, Flush};
use crate::keys;
use crate::keys::{Action, Scroll};
use crate::terminal::{Mode, Session};
use crate::ui;
use anyhow::Result;
use axum_ipc::{FrameReader, FrameWriter};
use axum_proto::{Cursor, HarnessEvent, UiCommand};
use axum_tui::footer::FooterData;
use axum_tui::{Theme, status, transcript};
use crossterm::event::{Event, EventStream};
use ratatui::text::Line;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// How long to wait before redialling a daemon that went away.
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// Run the UI until the user quits.
pub async fn run(socket: &Path, mode: Mode) -> Result<()> {
    let theme = Theme::default();
    let mut app = App::new();
    let footer_data = footer_data(mode);

    let mut session = Session::open(mode, ui::initial_height(terminal_size().1))?;
    let mut terminal_events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(status::FRAME_MS));

    let (event_tx, mut event_rx) = mpsc::channel::<HarnessEvent>(256);
    let (command_tx, command_rx) = mpsc::channel::<UiCommand>(32);
    tokio::spawn(connection_loop(
        socket.to_path_buf(),
        event_tx,
        command_rx,
        app.cursor(),
    ));

    let list_paths = |query: &str| {
        std::env::current_dir()
            .map(|cwd| crate::paths::list(&cwd, query))
            .unwrap_or_default()
    };

    let mut dirty = true;
    loop {
        if dirty {
            flush_settled(&mut session, &mut app, &theme)?;
            let _ = session.terminal.autoresize();
            let mode = session.mode;
            session
                .terminal
                .draw(|frame| ui::draw(frame, &mut app, &footer_data, &theme, mode))?;
            dirty = false;
        }

        tokio::select! {
            Some(event) = event_rx.recv() => {
                // A snapshot means the daemon accepted an attach: the socket is up.
                app.connected = true;
                app.apply(event);
                dirty = true;
            }
            Some(Ok(event)) = terminal_events.next() => {
                match event {
                    Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                        let busy = app.is_busy();
                        let action =
                            keys::handle(key, &mut app.editor, &mut app.completion, busy);
                        match action {
                            Action::Quit => break,
                            Action::Submit(text) => {
                                let _ = command_tx.send(UiCommand::SubmitPrompt { text }).await;
                                dirty = true;
                            }
                            Action::Command(text) => {
                                if run_command(&text, &mut app) == Control::Quit {
                                    break;
                                }
                                dirty = true;
                            }
                            Action::Interrupt => {
                                let _ = command_tx.send(UiCommand::Interrupt).await;
                                dirty = true;
                            }
                            Action::ExternalEdit => {
                                external_edit(&mut session, &mut app)?;
                                dirty = true;
                            }
                            Action::Scroll(motion) => {
                                // Inline mode has no owned buffer: the terminal's own
                                // scrollback already answers these keys, so the UI stays out
                                // of the way rather than fighting it.
                                if session.mode == Mode::Alt {
                                    let rows = terminal_size().1;
                                    let view = rows.saturating_sub(ui::CHROME_ROWS);
                                    match motion {
                                        Scroll::PageUp => app.scrollback.page_up(view),
                                        Scroll::PageDown => app.scrollback.page_down(view),
                                        Scroll::Top => app.scrollback.to_top(),
                                        Scroll::Bottom => app.scrollback.to_bottom(),
                                        Scroll::LineUp => app.scrollback.scroll_up(3),
                                        Scroll::LineDown => app.scrollback.scroll_down(3, view),
                                    }
                                    dirty = true;
                                }
                            }
                            Action::Redraw => dirty = true,
                            Action::Ignore => {}
                        }
                        // The popup is derived from the prompt, so it is recomputed after
                        // every key rather than mutated alongside the buffer.
                        app.refresh_completion(&list_paths);
                    }
                    Event::Paste(text) => {
                        app.editor.insert_str(&text);
                        app.refresh_completion(&list_paths);
                        dirty = true;
                    }
                    Event::Resize(_, _) => dirty = true,
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                if app.is_busy() {
                    app.tick = app.tick.wrapping_add(1);
                    dirty = true;
                }
            }
        }
    }

    Ok(())
}

/// Write settled transcript blocks into native scrollback.
///
/// `insert_before` scrolls the viewport down and emits the lines above it, so they become part
/// of the terminal's own history — searchable and copyable with the tools the user already has.
fn flush_settled(session: &mut Session, app: &mut App, theme: &Theme) -> Result<()> {
    // Alt mode owns the whole screen and the whole transcript; there is no terminal history to
    // hand anything to, and `insert_before` on a fullscreen viewport has nowhere to put it.
    if session.mode == Mode::Alt {
        return Ok(());
    }
    let Flush::Upto(n) = app.settled() else {
        return Ok(());
    };
    let width = terminal_width();
    let pending: Vec<Line<'static>> = app.entries()[..n]
        .iter()
        .skip(app.entries().len() - app.live().len())
        .flat_map(|entry| transcript::entry_lines(entry, width, theme))
        .collect();

    if !pending.is_empty() {
        let height = pending.len() as u16;
        session.terminal.insert_before(height, |buf| {
            ratatui::widgets::Widget::render(
                ratatui::widgets::Paragraph::new(pending.clone()),
                buf.area,
                buf,
            );
        })?;
    }
    app.mark_flushed(n);
    Ok(())
}

/// Keep a connection to the daemon, redialling when it drops.
///
/// A dead daemon is not an error for the UI: it is the detach case, and reattaching with the
/// last cursor is how an in-flight turn is rejoined rather than replayed.
async fn connection_loop(
    socket: std::path::PathBuf,
    events: mpsc::Sender<HarnessEvent>,
    mut commands: mpsc::Receiver<UiCommand>,
    mut from_cursor: Cursor,
) {
    loop {
        let Ok(stream) = axum_ipc::connect(&socket).await else {
            debug_log(format_args!("connect failed"));
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        };
        debug_log(format_args!("connected, attaching from {from_cursor:?}"));

        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);

        if writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor,
            })
            .await
            .is_err()
        {
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }

        // Reads run in their own task because `FrameReader::read` is not cancel-safe: it takes
        // a length and then a body, and a `select!` that drops it between the two leaves the
        // next read parsing body bytes as a length. Sending a command used to do exactly that,
        // which desynced the stream on the first prompt.
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

fn terminal_width() -> u16 {
    terminal_size().0
}

fn terminal_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// Footer content available before a daemon exists.
///
/// M1 replaces this with values the daemon reports; until then the UI states what it can see
/// for itself rather than inventing token counts.
fn footer_data(mode: Mode) -> FooterData {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let home = std::env::var("HOME").ok();
    FooterData {
        cwd: axum_tui::footer::format_cwd(&cwd, home.as_deref()),
        branch: git_branch(),
        model: "no-model".into(),
        mode: mode.label(),
        ..FooterData::default()
    }
}

fn git_branch() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|b| !b.is_empty())
}

/// Whether a slash command asked the UI to exit.
#[derive(Debug, PartialEq, Eq)]
enum Control {
    /// Stay running.
    Continue,
    /// Exit.
    Quit,
}

/// Run a slash command.
///
/// Every command here is answered locally. Anything that needs the daemon becomes a
/// [`UiCommand`] instead, so the set of things the UI can do alone stays visible in one match.
fn run_command(input: &str, app: &mut App) -> Control {
    match input.split_whitespace().next().unwrap_or_default() {
        "/quit" => Control::Quit,
        "/clear" => {
            app.clear_view();
            Control::Continue
        }
        "/help" => {
            app.show_help();
            Control::Continue
        }
        _ => {
            app.show_notice(format!("unknown command: {input}"));
            Control::Continue
        }
    }
}

/// Hand the prompt to `$EDITOR`, releasing the terminal for the duration.
///
/// The raw-mode session is dropped first and rebuilt after: a full-screen editor and a TUI
/// cannot share a tty, and leaving raw mode on would hand the editor unreadable input.
fn external_edit(session: &mut Session, app: &mut App) -> Result<()> {
    let mode = session.mode;
    let before = app.editor.text();
    let Some(editor) = crate::external_editor::editor_command() else {
        app.show_notice("no $EDITOR or $VISUAL is set".into());
        return Ok(());
    };

    let height = ui::initial_height(terminal_size().1);
    let placeholder = Session::open(mode, height)?;
    let previous = std::mem::replace(session, placeholder);
    drop(previous);

    let edited = crate::external_editor::edit_with(&editor, &before);

    *session = Session::open(mode, height)?;
    session.terminal.clear()?;

    match edited {
        Ok(Some(text)) => app.editor.set_text(&text),
        Ok(None) => {}
        Err(e) => app.show_notice(format!("editor failed: {e}")),
    }
    Ok(())
}

/// Append a line to `$AXUM_DEBUG_LOG`, if it is set.
///
/// A UI owns the terminal, so `eprintln!` is not available for diagnosis — it would land in
/// the middle of the frame. This is the only way to see what the loop actually did.
fn debug_log(args: std::fmt::Arguments<'_>) {
    let Some(path) = std::env::var_os("AXUM_DEBUG_LOG") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{args}");
    }
}
