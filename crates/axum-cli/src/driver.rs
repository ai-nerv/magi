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
use axum_tui::transcript;
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
///
/// `prompt` is the positional argument: `axum "…"` opens the UI with the question already
/// asked, so the first thing on screen is the answer arriving rather than an empty box the
/// user has to retype into.
pub async fn run(
    socket: &Path,
    mode: Mode,
    prompt: Option<String>,
    sessions: Option<std::path::PathBuf>,
) -> Result<()> {
    let mut app = App::new();
    // Read here rather than taken from the daemon, because this one is about the screen in front
    // of the person reading it. A model or a tool set has to come from the daemon — it is what
    // the daemon is actually using — but nothing on the other end of the socket has an opinion
    // about how fast a border moves. A config that will not run leaves the built-in speed: the
    // daemon has already refused to start over it and said why.
    if let Ok(loaded) = crate::config::load() {
        crate::config::adopt_ui(&loaded);
    }
    // Before anything else, because the answer to "why is my new tool not there" has to arrive
    // before the model is asked to use it. The daemon holds the tool set it was built with, and
    // a session that outlived a config edit reports the tool as unregistered -- which reads as a
    // broken tool rather than a stale daemon.
    let edited = crate::config::edited_since_start(socket);
    if !edited.is_empty() {
        let names: Vec<String> = edited
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        app.notice_after_attach(format!(
            "This session started before {} changed. Run `axum stop` to pick it up.",
            names.join(", ")
        ));
    }
    let base_footer = local_footer(mode);

    let mut session = Session::open(mode, ui::initial_height(terminal_size().1))?;
    let mut terminal_events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(axum_tui::metric::frame_ms()));

    let (event_tx, mut event_rx) = mpsc::channel::<HarnessEvent>(256);
    let (command_tx, command_rx) = mpsc::channel::<UiCommand>(32);
    // Shared rather than inferred from the event stream: a dropped connection produces no
    // event, so a UI watching only for events cannot tell "nothing is happening" from
    // "nothing can happen".
    let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    tokio::spawn(connection_loop(
        socket.to_path_buf(),
        event_tx,
        command_rx,
        sessions,
        app.cursor(),
        Arc::clone(&attached),
    ));

    let list_paths = |query: &str| {
        std::env::current_dir()
            .map(|cwd| crate::paths::list(&cwd, query))
            .unwrap_or_default()
    };

    // Sent once the connection task exists, not before: the channel buffers it, and it reaches
    // the daemon after the attach that the connection loop opens with.
    if let Some(text) = prompt {
        let _ = command_tx.send(UiCommand::SubmitPrompt { text }).await;
    }

    let mut dirty = true;
    loop {
        // Read each pass rather than tracked here: the connection lives in another task, and
        // this is the one thing about it the screen has to show.
        let attached_now = attached.load(Ordering::Relaxed);
        if attached_now != app.connected {
            app.connected = attached_now;
            dirty = true;
        }

        if dirty {
            flush_settled(&mut session, &mut app)?;
            let _ = session.terminal.autoresize();
            let mode = session.mode;
            session.terminal.draw(|frame| {
                let footer = footer_data(&base_footer, &app);
                app.queued = command_tx.max_capacity() - command_tx.capacity();
                ui::draw(frame, &mut app, &footer, mode);
            })?;
            dirty = false;
        }

        tokio::select! {
            Some(event) = event_rx.recv() => {
                app.apply(event);
                dirty = true;
            }
            Some(Ok(event)) = terminal_events.next() => {
                match event {
                    Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                        let busy = app.is_busy();
                        let action =
                            keys::handle(
                                key,
                                &mut app.editor,
                                &mut app.completion,
                                &mut app.picker,
                                busy,
                            );
                        // Noted before the match consumes it: a taken completion must not be
                        // recomputed, and the arms move the action's payload out.
                        let accepted =
                            action == Action::Accepted || action == Action::Dismissed;
                        match action {
                            Action::Quit => break,
                            Action::Submit(text) => {
                                let _ = command_tx.send(UiCommand::SubmitPrompt { text }).await;
                                dirty = true;
                            }
                            Action::Command(text) => {
                                match run_command(&text, &mut app) {
                                    Control::Quit => break,
                                    Control::Send(command) => {
                                        let _ = command_tx.send(command).await;
                                    }
                                    Control::Continue => {}
                                }
                                dirty = true;
                            }
                            Action::Interrupt => {
                                let _ = command_tx.send(UiCommand::Interrupt).await;
                                dirty = true;
                            }
                            Action::Chose(value) => {
                                let command = match app.picking.take() {
                                    Some(crate::app::Picking::Thinking) => {
                                        UiCommand::SetThinking { level: value }
                                    }
                                    // A list with no recorded purpose cannot have been opened
                                    // by anything here, so there is nothing to send.
                                    Some(crate::app::Picking::Model) => {
                                        UiCommand::SetModel { name: value }
                                    }
                                    // Matched back by *label*, because that is what the person
                                    // read and chose. The labels were generated from these same
                                    // scopes a moment ago, so the pairing is exact rather than
                                    // a guess — and a value matching none of them is the "no"
                                    // row, which is the only other thing in the list.
                                    Some(crate::app::Picking::Permission { id, offers }) => {
                                        let decision = offers
                                            .iter()
                                            .find(|scope| {
                                                scope.label(&app.asking_about) == value
                                            })
                                            .map_or(
                                                axum_proto::permit::Decision::Deny,
                                                |scope| axum_proto::permit::Decision::Allow {
                                                    scope: scope.clone(),
                                                    lifetime: axum_proto::permit::Lifetime::Session,
                                                },
                                            );
                                        UiCommand::Permit { id, decision }
                                    }
                                    None => continue,
                                };
                                let _ = command_tx.send(command).await;
                                dirty = true;
                            }
                            // Leaving a question is an answer to it. A permission prompt is the
                            // only list something is waiting on, and the wait is a turn that
                            // has stopped: closing it without a word left the daemon blocked
                            // until its own patience ran out, which on screen is a hang.
                            Action::Dismissed => {
                                if let Some(crate::app::Picking::Permission { id, .. }) =
                                    app.picking.take()
                                {
                                    let _ = command_tx
                                        .send(UiCommand::Permit {
                                            id,
                                            decision: axum_proto::permit::Decision::Deny,
                                        })
                                        .await;
                                }
                                dirty = true;
                            }
                            Action::ToggleDetail => {
                                // No notice. A view toggle is not something that happened in
                                // the conversation, and one line per press left a transcript
                                // that was half commentary after ten of them. What the fold
                                // is and how to undo it is written on the fold itself.
                                app.toggle_detail();
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
                                    let view = rows.saturating_sub(ui::chrome_rows());
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
                            Action::Redraw | Action::Accepted => dirty = true,
                            Action::Ignore => {}
                        }
                        // The popup is derived from the prompt, so it is recomputed after
                        // every key rather than mutated alongside the buffer -- except the
                        // key that just accepted one, which still matches what offered it.
                        // Not while a list is open: the popup is derived from the prompt, and
                        // the prompt is not what the arrows are about right now.
                        if !accepted && app.picker.is_none() {
                            app.refresh_completion(&list_paths);
                        }
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
                // Always, now. The spinner needed this only while something was running; the
                // prompt's border scan runs whenever the box is on screen, and a scan that
                // stops the moment a turn ends reads as the UI having frozen.
                app.advance();
                dirty = true;
            }
        }
    }

    // The daemon this UI started goes with it.
    //
    // A UI quitting was a detach and the daemon was left running, which is the right shape for
    // a long turn you want to walk away from and the wrong one for everything else: a week of
    // work left a process per project, each holding the socket, the tool set and the
    // environment of whichever shell happened to start it. That last one cost three sessions —
    // a daemon started before a key was exported can never see it, and nothing said so.
    //
    // **Unconditional.** The first attempt stopped only a daemon this UI had started itself,
    // which sounds careful and is the bug: anything left by an earlier `-p`, a crash, or a
    // killed session means the UI attaches rather than spawns, and it then left the mess
    // exactly where it found it. From the outside that is indistinguishable from the cleanup
    // never having been written.
    //
    // A second UI on the same socket survives it: its own reconnect loop starts a replacement
    // and resumes, because the session is on disk rather than in the process.
    //
    // The flag and the `axum.daemon` setting that would make this a choice are worth having and
    // are not worth guessing at before somebody wants the other behaviour.
    crate::stop::stop_one(&crate::daemon::pid_path(socket));

    Ok(())
}

/// Write settled transcript blocks into native scrollback.
///
/// `insert_before` scrolls the viewport down and emits the lines above it, so they become part
/// of the terminal's own history — searchable and copyable with the tools the user already has.
fn flush_settled(session: &mut Session, app: &mut App) -> Result<()> {
    // Whatever was in force when the block settled. Scrollback cannot be taken back, so a
    // later toggle changes what is drawn from here on and not what the terminal already holds.
    let detail = app.detail;
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
        .flat_map(|entry| transcript::entry_lines(entry, width, detail))
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
    sessions: Option<std::path::PathBuf>,
    mut from_cursor: Cursor,
    attached: Arc<std::sync::atomic::AtomicBool>,
) {
    loop {
        attached.store(false, Ordering::Relaxed);
        let Ok(stream) = axum_ipc::connect(&socket).await else {
            debug_log(format_args!("connect failed"));
            // Redialling alone only works when the daemon is slow, not when it is gone -- and
            // it is gone in every case that matters: it crashed, `axum stop` ended it, the
            // machine slept. The socket is removed on the way out, so there is nothing left to
            // dial and the UI spun on "Reconnecting" for as long as it was left open. Starting
            // one is what a detached UI is for.
            // `resume`, not a fresh session. The daemon owns the conversation and a restart is
            // meant to be invisible; starting a new one instead threw the transcript away and
            // the UI came back to a greeting, which is a worse failure than the hang this
            // replaced. The session being resumed is this directory's most recent, which is
            // precisely the one that just died.
            match crate::daemon::ensure(&socket, sessions.as_deref(), true).await {
                // Nothing to record: the UI stops this directory's daemon on the way out
                // whether it started it or adopted it.
                Ok(_) => {}
                Err(error) => {
                    debug_log(format_args!("restart failed: {error}"));
                    tokio::time::sleep(RECONNECT_DELAY).await;
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

fn terminal_width() -> u16 {
    terminal_size().0
}

fn terminal_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// What the footer can say without asking anyone.
///
/// The working directory and the branch are facts about this process. Everything else — which
/// model, how many tokens, how full the window is — belongs to the daemon, and is filled in by
/// [`footer_data`] once it has told us.
fn local_footer(mode: Mode) -> FooterData {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let home = std::env::var("HOME").ok();
    FooterData {
        cwd: axum_tui::footer::format_cwd(&cwd, home.as_deref()),
        branch: git_branch(),
        model: axum_tui::glyph::no_model().into(),
        // Named only for the backend that is not the default. `alt` is what you get unless you
        // asked otherwise, so saying so on every line is a word that is never news; `inline`
        // is a choice you made and worth confirming.
        mode: if matches!(mode, Mode::Inline) {
            mode.label()
        } else {
            ""
        },
        ..FooterData::default()
    }
}

/// The footer as of now.
///
/// Rebuilt each frame from what the daemon has reported rather than kept in step by hand: the
/// numbers change on every delta, and a copy updated at each of the places that could change
/// them is a copy that misses one.
fn footer_data(base: &FooterData, app: &App) -> FooterData {
    let window = app.model.as_ref().map_or(0, |m| m.context_window);
    FooterData {
        cwd: base.cwd.clone(),
        branch: base.branch.clone(),
        mode: base.mode,
        model: app.model.as_ref().map_or_else(
            || axum_tui::glyph::no_model().to_owned(),
            |m| m.name.clone(),
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
#[derive(Debug, PartialEq)]
enum Control {
    /// Stay running.
    Continue,
    /// Exit.
    Quit,
    /// Something only the daemon can do.
    Send(UiCommand),
}

/// Run a slash command.
///
/// Every command here is answered locally. Anything that needs the daemon becomes a
/// [`UiCommand`] instead, so the set of things the UI can do alone stays visible in one match.
fn run_command(input: &str, app: &mut App) -> Control {
    match input.split_whitespace().next().unwrap_or_default() {
        "/quit" => Control::Quit,
        // Both halves, because the name promises both. Clearing only the view left the model
        // remembering everything while the footer reported an empty context -- the screen and
        // the token count both lying, in the same direction, at the same time. The branch is
        // journalled, so the record of what was said survives what the model is shown.
        "/clear" => {
            app.clear_view();
            Control::Send(UiCommand::Branch { keeps: Some(0) })
        }
        "/help" => {
            app.show_help();
            Control::Continue
        }
        // With a name it is the daemon's to do: only it knows the catalog this session
        // started with and whether the name reaches anything. Without one, the answer is
        // already on screen — the footer says which model is answering — so this says it
        // again in words, which is what somebody typing `/model` is asking for.
        "/model" => match input.split_whitespace().nth(1) {
            Some(name) => Control::Send(UiCommand::SetModel {
                name: name.to_owned(),
            }),
            // A list rather than a sentence. Somebody asking this has usually configured
            // nothing, and being told "no model is configured" answers the question they did
            // not ask while leaving the one they did.
            None => {
                app.open_model_picker();
                Control::Continue
            }
        },
        // Same shape as `/model`: a list rather than a sentence, because the useful reply to
        // "how much reasoning" is the set of answers and which of them this model can give.
        "/think" => match input.split_whitespace().nth(1) {
            Some(level) => Control::Send(UiCommand::SetThinking {
                level: level.to_owned(),
            }),
            None => {
                app.open_thinking_picker();
                Control::Continue
            }
        },
        // The daemon's, because it holds the conversation the question is about and the
        // provider that answers it.
        "/permissions" => Control::Send(UiCommand::DeclareNeeds),
        // Rewinding is the daemon's to work out: it holds the session, and which messages are
        // still live is a question about the session rather than about what is on screen.
        "/rewind" => match input.split_whitespace().nth(1) {
            None => Control::Send(UiCommand::Branch { keeps: None }),
            Some(n) => match n.parse() {
                Ok(keeps) => Control::Send(UiCommand::Branch { keeps: Some(keeps) }),
                Err(_) => {
                    app.show_notice(format!("/rewind takes a number, not {n:?}"));
                    Control::Continue
                }
            },
        },
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
