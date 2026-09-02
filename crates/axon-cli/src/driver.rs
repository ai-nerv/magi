//! The UI event loop.
//!
//! Three sources feed one `select!`: the socket, the terminal, and a spinner timer. State
//! lives in [`App`], drawing lives in [`ui`], and this file owns only the wiring — which is
//! what keeps the loop small enough to read.

use crate::app::App;
use crate::keys;
use crate::keys::{Action, Scroll};
use crate::terminal::Session;
use crate::ui;
use anyhow::Result;
use axon_ipc::{FrameReader, FrameWriter};
use axon_proto::{Cursor, HarnessEvent, UiCommand};
use axon_tui::footer::FooterData;
use crossterm::event::{Event, EventStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// How long to wait before redialling a session that went away.
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// Run the UI until the user quits.
///
/// `prompt` is the positional argument: `axon "…"` opens the UI with the question already
/// asked, so the first thing on screen is the answer arriving rather than an empty box the
/// user has to retype into.
pub async fn run(
    socket: &Path,
    prompt: Option<String>,
    loaded: Option<crate::config::Loaded>,
    project: &str,
    started: Option<(crate::atom::Atom, std::path::PathBuf)>,
) -> Result<()> {
    // **Before anything reads a setting.** `colour`, `glyph` and `metric` each hold their table
    // in a `OnceLock` that the first *read* fills with the built-in defaults, and `adopt` after
    // that is a no-op. `App::new` reads one — it needs a line for the empty prompt — so building
    // it first threw the whole configured `axon.ui` away, silently, and left the box repeating
    // the one placeholder compiled into the binary.
    //
    // Read here rather than taken from the session, because this is about the screen in front of
    // the person reading it. A model or a tool set has to come from the session — it is what the
    // session is actually using — but nothing on the other end of the socket has an opinion about
    // how fast a border moves.
    if let Some(loaded) = &loaded {
        crate::config::adopt_ui(loaded);
    }
    let mut app = App::new();
    // What the footer shows. atom names a session because it can see the namespace and axon
    // cannot; without atom there are no siblings to be told apart, so the project is name enough.
    app.named = started
        .as_ref()
        .map_or_else(|| project.to_owned(), |(layer, _)| layer.named.clone());
    // The prompts from previous runs, so the arrow keys reach past this one.
    app.editor = axon_tui::Editor::with_history(crate::history::load());
    if let Some(loaded) = &loaded {
        // Worked out here because the answer needs the catalog, and the snapshot carries only
        // whether there is a model — not why there is not. Same text the session refuses a prompt
        // with, so meeting the problem at attach and meeting it at the first prompt say one thing.
        if crate::config::backend(loaded).is_none() {
            app.no_model = Some(axon_host::no_model(&crate::config::catalog(loaded)));
        }
    }
    // Before anything else, because the answer to "why is my new tool not there" has to arrive
    // before the model is asked to use it. A session holds the tool set it was built with, and
    // one that has been open across a config edit reports the tool as unregistered -- which
    // reads as a broken tool rather than as a session that predates it.
    let edited = crate::config::edited_since_start(socket);
    if !edited.is_empty() {
        let names: Vec<String> = edited
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        app.notice_after_attach(format!(
            "This session started before {} changed. Quit and start axon again to pick it up.",
            names.join(", ")
        ));
    }
    let mut session = Session::open()?;
    // From here, not from the start of `main`: the clock is for the screen, and there is no
    // screen until the alternate one is open.
    axon_tui::decrypt::begin();
    let mut terminal_events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(axon_tui::metric::frame_ms()));

    let (event_tx, mut event_rx) = mpsc::channel::<HarnessEvent>(256);
    let (command_tx, command_rx) = mpsc::channel::<UiCommand>(32);
    // Shared rather than inferred from the event stream: a dropped connection produces no
    // event, so a UI watching only for events cannot tell "nothing is happening" from
    // "nothing can happen".
    let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // What atom says, on its way to becoming an entry. Read on a thread because it is a
    // blocking pipe and everything else in this loop is not — and dropped, along with atom
    // itself, when this function returns.
    let (heard_tx, mut heard) = mpsc::channel::<crate::atom::Heard>(64);
    let mut layer = started.map(|(layer, _at)| layer);
    if let Some(reading) = layer.as_mut().and_then(crate::atom::Atom::hearing) {
        std::thread::spawn(move || {
            use std::io::BufRead;
            // Already buffered, and handed over as the reader for that reason: the line that
            // named this session was read through it, and whatever followed the newline is
            // sitting in it. Wrapping the raw pipe again here would have thrown that away.
            for line in reading.lines().map_while(Result::ok) {
                // A line this build cannot read is a newer atom, not a reason to stop reading:
                // the next one may well be a message, and dropping the whole pipe over a field
                // nobody here knows about would lose it.
                let Ok(said) = serde_json::from_str::<crate::atom::Heard>(&line) else {
                    continue;
                };
                if heard_tx.blocking_send(said).is_err() {
                    return;
                }
            }
        });
    }

    tokio::spawn(connection_loop(
        socket.to_path_buf(),
        event_tx,
        command_rx,
        app.cursor(),
        Arc::clone(&attached),
    ));

    let list_paths = |query: &str| {
        std::env::current_dir()
            .map(|cwd| crate::paths::list(&cwd, query))
            .unwrap_or_default()
    };

    // Sent once the connection task exists, not before: the channel buffers it, and it reaches
    // the session after the attach that the connection loop opens with.
    if let Some(text) = prompt {
        let _ = command_tx
            .send(UiCommand::SubmitPrompt {
                text,
                aside: String::new(),
            })
            .await;
    }

    let mut dirty = true;
    // What shape the terminal was last told to draw its cursor in. Insert mode is a bar and
    // normal mode a block, which is the one cue that says which mode you are in without
    // looking away from what you are typing.
    let mut shown = axon_tui::vim::Mode::Insert;
    // Set by a mouse release, acted on after the next draw: the text a selection covers is read
    // back out of the frame it was drawn into, so there has to be a frame.
    let mut copied: Option<axon_tui::select::Selection> = None;
    // Whether a turn was running last frame. A turn *ending* is the edge that answers an
    // arrival, and neither side of the pipe can see it: atom cannot see a turn at all, and the
    // session publishes what it is doing rather than what it just stopped doing.
    let mut was_busy = false;
    loop {
        // Read each pass rather than tracked here: the connection lives in another task, and
        // this is the one thing about it the screen has to show.
        let attached_now = attached.load(Ordering::Relaxed);
        if attached_now != app.connected {
            app.connected = attached_now;
            dirty = true;
        }

        if dirty {
            let _ = session.terminal.autoresize();
            session.terminal.draw(|frame| {
                let footer = footer_data(&app);
                app.queued = command_tx.max_capacity() - command_tx.capacity();
                ui::draw(frame, &mut app, &footer);
            })?;
            dirty = false;
            // After the frame, and only when it has changed: the shape is the terminal's own
            // cursor, so it outlives a redraw and does not need setting on every one.
            if shown != app.modal.mode {
                shown = app.modal.mode;
                let _ = crossterm::execute!(std::io::stdout(), crate::terminal::shape(shown));
            }
            if let Some(sel) = copied.take() {
                let area = session.terminal.get_frame().area();
                let text = axon_tui::select::text(session.terminal.current_buffer_mut(), sel, area);
                if !text.is_empty() {
                    crate::clipboard::put(&text);
                }
            }
        }

        tokio::select! {
            Some(event) = event_rx.recv() => {
                app.apply(event);
                dirty = true;
            }
            Some(Ok(event)) = terminal_events.next() => {
                match event {
                    Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                        // Somebody is here. Whatever the box was writing to itself, it stops and
                        // starts its wait over -- including on the keys that leave the prompt
                        // empty, which is most of them at this point.
                        app.tease.interrupt();
                        let busy = app.is_busy();
                        let action = keys::handle(
                            key,
                            &mut app.editor,
                            &mut app.overlay,
                            busy,
                            &mut app.modal,
                        );
                        // Noted before the match consumes it: a taken completion must not be
                        // recomputed, and the arms move the action's payload out.
                        let accepted = matches!(
                            action,
                            Action::Accepted | Action::Dismissed | Action::Recalled
                        );
                        match action {
                            Action::Submit(text) => {
                                crate::history::remember(&text);
                                // A prompt that names another instance is still the model's to
                                // answer. What naming one does is *tell the model it is there*
                                // and that there is a tool for reaching it -- axon delivering
                                // the message itself would be the harness deciding what the
                                // model meant by "tell", which is the model's job.
                                //
                                // Beside the prompt, not appended to it. It used to be spliced
                                // onto the end under a rule, so typing "ask $iota-mu about the
                                // parser" put a page of facts about `iota-mu` into the
                                // transcript under your own name. You typed one line; you
                                // should see one line.
                                let aside = layer
                                    .as_ref()
                                    .map_or_else(String::new, |l| l.briefing(&text, project));
                                let _ = command_tx
                                    .send(UiCommand::SubmitPrompt { text, aside })
                                    .await;
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
                            // Search is the next thing to be built; until it is, these move
                            // nothing and say nothing rather than pretending to.
                            Action::Search | Action::Match { .. } => {}
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
                                    // Matched back by position, because a row is labelled with
                                    // what a person can read -- when it was, what they asked --
                                    // and none of that is the id the session needs.
                                    Some(crate::app::Picking::Session { rows }) => {
                                        let found = rows
                                            .iter()
                                            .find(|(label, _)| *label == value)
                                            .map(|(_, id)| id.clone());
                                        match found {
                                            Some(id) => UiCommand::Resume { id },
                                            None => continue,
                                        }
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
                                                axon_proto::permit::Decision::Deny,
                                                |scope| axon_proto::permit::Decision::Allow {
                                                    scope: scope.clone(),
                                                    lifetime: axon_proto::permit::Lifetime::Session,
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
                            // has stopped: closing it without a word left the session blocked
                            // until its own patience ran out, which on screen is a hang.
                            Action::Dismissed => {
                                if let Some(crate::app::Picking::Permission { id, .. }) =
                                    app.picking.take()
                                {
                                    let _ = command_tx
                                        .send(UiCommand::Permit {
                                            id,
                                            decision: axon_proto::permit::Decision::Deny,
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
                            Action::Redraw | Action::Accepted | Action::Recalled => dirty = true,
                            Action::Ignore => {}
                        }
                        // The popup is derived from the prompt, so it is recomputed after
                        // every key rather than mutated alongside the buffer -- except the
                        // key that just accepted one, which still matches what offered it.
                        // Not while a list is open: the popup is derived from the prompt, and
                        // the prompt is not what the arrows are about right now.
                        if !accepted
                            && !app
                                .overlay
                                .as_ref()
                                .is_some_and(axon_tui::overlay::Overlay::is_picker)
                        {
                            app.refresh_completion(&list_paths);
                        }
                    }
                    // axon never turns mouse reporting on -- a terminal that has handed the mouse
                    // to an application stops selecting text everywhere. These arrive only if
                    // something between here and the terminal sends them anyway, and a
                    // multiplexer that does its own selection may well. Answered rather than
                    // dropped: the events are free, and refusing them buys nothing back.
                    Event::Mouse(mouse) => {
                        use crossterm::event::MouseEventKind;
                        let rows = terminal_size().1;
                        let view = rows.saturating_sub(ui::chrome_rows());
                        match mouse.kind {
                            MouseEventKind::ScrollUp => app.scrollback.scroll_up(3),
                            MouseEventKind::ScrollDown => app.scrollback.scroll_down(3, view),
                            // A tool block opens and closes under the pointer. Ctrl+O still
                            // moves the whole transcript at once; this is for the one result
                            // you actually want to read, which is usually not the newest.
                            // The handle first: it is the one thing on screen that is a button,
                            // and a press on it is a press on it rather than the start of a
                            // one-character selection.
                            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                app.selection = None;
                                if !app.toggle_at(mouse.row, mouse.column, terminal_size().0) {
                                    app.selection =
                                        Some(axon_tui::select::Selection::begin(mouse.row, mouse.column));
                                }
                            }
                            MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                                if let Some(sel) = app.selection.as_mut() {
                                    sel.drag_to(mouse.row, mouse.column);
                                } else {
                                    continue;
                                }
                            }
                            // Copied on release, because that is when a person has finished
                            // choosing. Through OSC 52, which is the clipboard a terminal will
                            // accept through a multiplexer and over ssh alike.
                            MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                                let Some(sel) = app.selection.as_mut() else {
                                    continue;
                                };
                                sel.drag_to(mouse.row, mouse.column);
                                sel.finish();
                                if sel.is_empty() {
                                    app.selection = None;
                                } else {
                                    copied = app.selection;
                                }
                            }
                            _ => continue,
                        }
                        dirty = true;
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
                // What the socket is allowed to say about us, and what it heard. Done on the
                // frame rather than where the state changes: the socket answers with whatever
                // was last published, and a session that only republished on some paths would
                // report a turn that finished a minute ago.
                // What atom heard, and what it is allowed to say about us. Done on the frame
                // rather than where the state changes: atom answers with whatever it was last
                // told, and a session that only told it on some paths would report a turn that
                // finished a minute ago.
                let mut ended = false;
                while let Ok(said) = heard.try_recv() {
                    match said {
                        // Handed to the session, not drawn here. atom is where a message lands,
                        // but the transcript and the turns are the session's — an entry the UI
                        // appended for itself is one the model never sees, and an instance
                        // could be asked a question and sit there until somebody typed at it.
                        crate::atom::Heard::Message { who, sort, text } => {
                            let _ = command_tx.send(app.received(&who, &sort, &text)).await;
                        }
                        crate::atom::Heard::Around { names } => app.reachable = names,
                        crate::atom::Heard::Stopped => ended = true,
                        // Said once, at startup, and read there. A second one is a newer atom
                        // saying something this build has no use for.
                        crate::atom::Heard::Listening { .. } => {}
                    }
                }
                if ended {
                    break;
                }
                // The turn that was running has finished, so whatever it was answering has been
                // answered. Counted rather than matched up one for one: a turn answers whatever
                // arrived before it, and a sibling asking `status` wants to know whether it is
                // still waiting, not which of its messages this was.
                if was_busy && !app.is_busy() {
                    app.answered();
                }
                was_busy = app.is_busy();
                if let Some(layer) = layer.as_mut() {
                    layer.doing(
                        app.is_busy(),
                        app.elapsed().map_or(0, |since| since.as_secs()),
                        app.unanswered(),
                    );
                }
                dirty = true;
            }
        }
    }

    Ok(())
}

/// Keep a connection to the session, redialling when it drops.
///
/// A dead session is not an error for the UI: it is the detach case, and reattaching with the
/// last cursor is how an in-flight turn is rejoined rather than replayed.
async fn connection_loop(
    socket: std::path::PathBuf,
    events: mpsc::Sender<HarnessEvent>,
    mut commands: mpsc::Receiver<UiCommand>,
    mut from_cursor: Cursor,
    attached: Arc<std::sync::atomic::AtomicBool>,
) {
    loop {
        attached.store(false, Ordering::Relaxed);
        let Ok(stream) = axon_ipc::connect(&socket).await else {
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

fn terminal_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// The footer as of now.
///
/// Rebuilt each frame from what the session has reported rather than kept in step by hand: the
/// numbers change on every delta, and a copy updated at each of the places that could change
/// them is a copy that misses one.
fn footer_data(app: &App) -> FooterData {
    let window = app.model.as_ref().map_or(0, |m| m.context_window);
    FooterData {
        identity: app.named.clone(),
        model: app.model.as_ref().map_or_else(
            || axon_tui::glyph::no_model().to_owned(),
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

/// Whether a slash command asked the UI to exit.
#[derive(Debug, PartialEq)]
enum Control {
    /// Stay running.
    Continue,
    /// Exit.
    Quit,
    /// Something only the session can do.
    Send(UiCommand),
}

/// Run a colon command.
///
/// Every command here is answered locally. Anything that needs the session becomes a
/// [`UiCommand`] instead, so the set of things the UI can do alone stays visible in one match.
fn run_command(input: &str, app: &mut App) -> Control {
    match input.split_whitespace().next().unwrap_or_default() {
        ":quit" | ":q" => Control::Quit,
        // Both halves, because the name promises both. Clearing only the view left the model
        // remembering everything while the footer reported an empty context -- the screen and
        // the token count both lying, in the same direction, at the same time. The branch is
        // journalled, so the record of what was said survives what the model is shown.
        ":clear" => {
            app.clear_view();
            Control::Send(UiCommand::Branch { keeps: Some(0) })
        }
        ":help" => {
            app.show_help();
            Control::Continue
        }
        // With a name it is the session's to do: only it knows the catalog this session
        // started with and whether the name reaches anything. Without one, the answer is
        // already on screen — the footer says which model is answering — so this says it
        // again in words, which is what somebody typing `:model` is asking for.
        ":model" => match input.split_whitespace().nth(1) {
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
        // Same shape as `:model`: a list rather than a sentence, because the useful reply to
        // "how much reasoning" is the set of answers and which of them this model can give.
        ":think" => match input.split_whitespace().nth(1) {
            Some(level) => Control::Send(UiCommand::SetThinking {
                level: level.to_owned(),
            }),
            None => {
                app.open_thinking_picker();
                Control::Continue
            }
        },
        // The session's, because it holds the conversation the question is about and the
        // provider that answers it.
        ":permissions" => Control::Send(UiCommand::DeclareNeeds),
        // A list rather than a flag. `--resume` continues this directory's most recent session
        // and there was no way to reach any of the others, which is most of them.
        ":resume" => {
            app.open_session_picker();
            Control::Continue
        }
        // Rewinding is the session's to work out: it holds the session, and which messages are
        // still live is a question about the session rather than about what is on screen.
        ":rewind" => match input.split_whitespace().nth(1) {
            None => Control::Send(UiCommand::Branch { keeps: None }),
            Some(n) => match n.parse() {
                Ok(keeps) => Control::Send(UiCommand::Branch { keeps: Some(keeps) }),
                Err(_) => {
                    app.show_notice(format!(":rewind takes a number, not {n:?}"));
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

/// Append a line to `$AXON_DEBUG_LOG`, if it is set.
///
/// A UI owns the terminal, so `eprintln!` is not available for diagnosis — it would land in
/// the middle of the frame. This is the only way to see what the loop actually did.
fn debug_log(args: std::fmt::Arguments<'_>) {
    let Some(path) = std::env::var_os("AXON_DEBUG_LOG") else {
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
