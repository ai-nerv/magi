//! The UI event loop. Three sources feed one `select!`: the socket, the terminal, and a spinner
//! timer. State lives in [`App`], drawing lives in [`ui`], and this file owns only the wiring.

use crate::app::App;
use crate::keys;
use crate::keys::{Action, Scroll};
use crate::terminal::Session;
use crate::ui;
use anyhow::Result;
use crossterm::event::{Event, EventStream};
use magi_proto::{HarnessEvent, UiCommand};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// How long to wait before redialling a session that went away.
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// Run the UI until the user quits. `prompt` is the positional argument: `magi "…"` opens the UI
/// with the question already asked.
pub async fn run(
    socket: &Path,
    prompt: Option<String>,
    loaded: Option<crate::config::Loaded>,
    project: &str,
    started: Option<(crate::melchior::Melchior, std::path::PathBuf)>,
) -> Result<()> {
    // Before anything reads a setting: `colour`, `glyph` and `metric` each hold their table in a
    // `OnceLock` the first read fills with defaults, and `adopt` after that is a no-op.
    if let Some(loaded) = &loaded {
        crate::config::adopt_ui(loaded);
    }
    let mut app = App::new();
    // What the configuration already allows, so a session taking a child on can lend it at once.
    if let Some(loaded) = &loaded {
        app.granted = crate::config::granted(loaded);
    }
    // melchior names a session because it can see the namespace and magi cannot.
    app.named = started
        .as_ref()
        .map_or_else(|| project.to_owned(), |(layer, _)| layer.named.clone());
    app.editor = magi_tui::Editor::with_history(crate::history::load());
    if let Some(loaded) = &loaded {
        // The snapshot carries only whether there is a model; the answer needs the catalog.
        let catalog = crate::config::catalog(
            loaded,
            magi_host::broker::cards(&crate::config::mind(loaded)).await,
        );
        if crate::config::backend(&catalog).is_none() {
            app.no_model = Some(magi_host::no_model(&catalog));
        }
    }
    // A session holds the tool set it was built with, and one open across a config edit reports a
    // new tool as unregistered — which reads as a broken tool rather than a stale session.
    let edited = crate::config::edited_since_start(socket);
    if !edited.is_empty() {
        let names: Vec<String> = edited
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        app.notice_after_attach(format!(
            "This session started before {} changed. Quit and start magi again to pick it up.",
            names.join(", ")
        ));
    }
    let mut session = Session::open()?;
    // From here, not from the start of `main`: there is no screen until the alternate one is open.
    magi_tui::decrypt::begin();
    let mut terminal_events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(magi_tui::metric::frame_ms()));

    let (event_tx, mut event_rx) = mpsc::channel::<HarnessEvent>(256);
    let (command_tx, command_rx) = mpsc::channel::<UiCommand>(32);
    // A dropped connection produces no event, so events alone cannot tell "nothing is happening"
    // from "nothing can happen".
    let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Read on a thread because it is a blocking pipe, and dropped with melchior when this returns.
    let (heard_tx, mut heard) = mpsc::channel::<crate::melchior::Heard>(64);
    let mut layer = started.map(|(layer, _at)| layer);
    if let Some(reading) = layer.as_mut().and_then(crate::melchior::Melchior::hearing) {
        std::thread::spawn(move || {
            use std::io::BufRead;
            // Already buffered, and handed over for that reason: the line that named this session
            // was read through it, and whatever followed the newline is sitting in it.
            for line in reading.lines().map_while(Result::ok) {
                // A line this build cannot read is a newer melchior, not a reason to stop reading.
                let Ok(said) = serde_json::from_str::<crate::melchior::Heard>(&line) else {
                    continue;
                };
                if heard_tx.blocking_send(said).is_err() {
                    return;
                }
            }
        });
    }

    // A watch, not a channel: somebody who pressed the arrow four times wants the fourth agent.
    let (target_tx, target_rx) = tokio::sync::watch::channel(socket.to_path_buf());
    tokio::spawn(connection_loop(
        socket.to_path_buf(),
        target_rx,
        event_tx,
        command_rx,
        app.cursor(),
        Arc::clone(&attached),
    ));
    // What melchior handed this session while the screen was somewhere else. See `crewing::ours`.
    let mut held: Vec<UiCommand> = Vec::new();

    let list_paths = |query: &str| {
        std::env::current_dir()
            .map(|cwd| crate::paths::list(&cwd, query))
            .unwrap_or_default()
    };

    // Sent once the connection task exists: the channel buffers it until after the attach.
    if let Some(text) = prompt {
        let _ = command_tx
            .send(UiCommand::SubmitPrompt {
                text,
                aside: String::new(),
            })
            .await;
    }

    let mut dirty = true;
    // A surface reading a hold needs releases on keys that produce text, and asking for those
    // globally stops `:` opening the command line — so the layer goes on only while one has it.
    let mut keys_held = false;
    // Two escapes in a row take the screen back, so `esc` itself stays a key the tenant can read.
    let mut escaped = false;
    // Insert mode is a bar and normal mode a block, the one cue that says which mode you are in.
    let mut shown = magi_tui::vim::Mode::Insert;
    // Set by a mouse release, acted on after the next draw: the text is read back out of the frame.
    let mut copied: Option<magi_tui::select::Selection> = None;
    // A turn ending is the edge that answers an arrival, and neither side of the pipe can see it.
    let mut was_busy = false;
    // Compared rather than sent every frame, so a redraw per keystroke is not a command per keystroke.
    let mut told_room = None;
    loop {
        // Read each pass rather than tracked here: the connection lives in another task.
        let attached_now = attached.load(Ordering::Relaxed);
        if attached_now != app.connected {
            app.connected = attached_now;
            dirty = true;
        }
        // Compared each pass because a surface can end several ways, all of which free the keyboard.
        let holding_now = app.holding().is_some();
        if holding_now != keys_held {
            crate::terminal::hold_keys(holding_now);
            keys_held = holding_now;
        }

        if dirty {
            let _ = session.terminal.autoresize();
            let mut room = told_room.unwrap_or_default();
            let drawn = session.terminal.draw(|frame| {
                let footer = footer_data(&app);
                app.queued = command_tx.max_capacity() - command_tx.capacity();
                room = ui::draw(frame, &mut app, &footer);
            })?;
            // Measured in the draw, told after it: the session has no terminal.
            if told_room != Some(room) {
                told_room = Some(room);
                direct(
                    &mut app,
                    &command_tx,
                    UiCommand::Sized {
                        rows: Some(room),
                        cols: inner(),
                        holds: crate::terminal::reports_holds(),
                    },
                )
                .await;
            }
            // Read out of the frame that was just drawn, which is what `draw` hands back. Not
            // `current_buffer_mut`: ratatui ends every draw with `swap_buffers`, which resets the
            // one it is about to make current, so the text taken from it is always empty.
            let copy = copied
                .take()
                .map(|sel| magi_tui::select::text(drawn.buffer, sel, drawn.area))
                .filter(|text| !text.is_empty());
            dirty = false;
            // Only when it has changed: the shape is the terminal's own cursor and outlives a redraw.
            if shown != app.modal.mode {
                shown = app.modal.mode;
                let _ = crossterm::execute!(std::io::stdout(), crate::terminal::shape(shown));
            }
            if let Some(text) = copy {
                crate::clipboard::put(&text);
            }
        }

        tokio::select! {
            Some(event) = event_rx.recv() => {
                app.apply(event);
                dirty = true;
            }
            Some(Ok(event)) = terminal_events.next() => {
                match event {
                    // Every kind of key event: with the Kitty protocol a held key arrives as `Repeat`.
                    Event::Key(key) => {
                        // Whatever the box was writing to itself, it stops and starts its wait over.
                        app.tease.interrupt();
                        // How magi learns the protocol is live; the startup probe was only a guess.
                        if matches!(
                            key.kind,
                            crossterm::event::KeyEventKind::Repeat
                                | crossterm::event::KeyEventKind::Release
                        ) && crate::terminal::noticed_hold()
                        {
                            direct(
                                &mut app,
                                &command_tx,
                                UiCommand::Sized {
                                    rows: None,
                                    cols: inner(),
                                    holds: true,
                                },
                            )
                            .await;
                        }
                        // A surface has the keyboard while it has the rows, forwarded by name and
                        // never interpreted. Escape twice takes the screen back: a single one is
                        // forwarded, because a tenant can hold a pty where `esc` is the program's.
                        if let Some(held) = app.holding() {
                            let id = held.id.clone();
                            if key.kind != crossterm::event::KeyEventKind::Release {
                                if key.code == crossterm::event::KeyCode::Esc {
                                    if escaped {
                                        app.surface = None;
                                    }
                                    escaped = true;
                                } else {
                                    escaped = false;
                                }
                            }
                            if let Some(named) = crate::keying::named(key) {
                                // A list stepping two rows for one press is a release read as a
                                // press, or the same press twice — bugs in different programs.
                                debug_log(format_args!(
                                    "surface key {named} {:?}",
                                    crate::keying::held(key)
                                ));
                                direct(
                                    &mut app,
                                    &command_tx,
                                    UiCommand::Keyed {
                                        id,
                                        key: named,
                                        state: crate::keying::held(key),
                                    },
                                )
                                .await;
                            }
                            continue;
                        }
                        // A repeat it has: with the protocol on, holding backspace arrives as repeats.
                        if key.kind == crossterm::event::KeyEventKind::Release {
                            continue;
                        }
                        // `:trace` and `:cost` are modal for the keyboard, which is wrong when a turn
                        // has stopped: the picker would be underneath and Enter would never arrive.
                        if app.questioned() {
                            app.pane = None;
                        }
                        let busy = app.is_busy();
                        let page = magi_tui::pane::Pane::page(ratatui::layout::Rect {
                            x: 0,
                            y: 0,
                            width: terminal_size().0,
                            height: terminal_size().1,
                        });
                        let action = keys::handle(
                            key,
                            &mut app.editor,
                            &mut app.overlay,
                            &mut app.pane,
                            page,
                            busy,
                            &mut app.modal,
                        );
                        // Noted before the match consumes it; the rule lives in `keys::recomputes`.
                        let accepted = !keys::recomputes(&action);
                        match action {
                            // Given back, not swallowed: `submit` empties the box before the gate.
                            Action::Submit(text) if app.attached.is_some() => {
                                app.refuse_drive();
                                app.editor.insert_str(&text);
                                dirty = true;
                            }
                            Action::Submit(text) => {
                                crate::history::remember(&text);
                                // Beside the prompt, not appended to it: naming an instance tells the
                                // model it is there and that a tool reaches it.
                                let aside = layer
                                    .as_ref()
                                    .map_or_else(String::new, |l| l.briefing(&text, project));
                                direct(
                                    &mut app,
                                    &command_tx,
                                    UiCommand::SubmitPrompt { text, aside },
                                )
                                .await;
                                dirty = true;
                            }
                            Action::Command(text) => {
                                match run_command(&text, &mut app) {
                                    Control::Quit => break,
                                    Control::Send(command) => {
                                        direct(&mut app, &command_tx, command).await;
                                    }
                                    Control::Continue => {}
                                }
                                dirty = true;
                            }
                            // Search is the next thing to be built; until it is, these move nothing.
                            Action::Search | Action::Match { .. } => {}
                            Action::Interrupt => {
                                direct(&mut app, &command_tx, UiCommand::Interrupt).await;
                                dirty = true;
                            }
                            // This replaces the lot — see `App::attach_to`, where the forgetting is.
                            Action::Crew { forward } => {
                                if walk(
                                    &mut app,
                                    forward,
                                    socket,
                                    &target_tx,
                                    &command_tx,
                                    &mut held,
                                )
                                .await
                                {
                                    dirty = true;
                                }
                            }
                            Action::Chose(value) => {
                                // Answered down the pipe, not over the socket, so it is taken first.
                                if let Some(crate::app::Picking::Adoption { id }) =
                                    app.picking.as_ref()
                                {
                                    let (id, accept) = (id.clone(), value == "yes");
                                    // What was consented to is what was on the table then.
                                    let lending = accept.then(|| app.lending());
                                    app.picking = None;
                                    if let Some(layer) = layer.as_mut() {
                                        layer.answered(&id, accept, lending.as_deref());
                                    }
                                    dirty = true;
                                    continue;
                                }
                                let command = match app.picking.take() {
                                    Some(crate::app::Picking::Thinking) => {
                                        UiCommand::SetThinking { level: value }
                                    }
                                    // No recorded purpose, so nothing here opened it and nothing goes.
                                    Some(crate::app::Picking::Model) => {
                                        UiCommand::SetModel { name: value }
                                    }
                                    // Matched back by position: a row is labelled for a person to
                                    // read, and none of that is the id the session needs.
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
                                    // Matched back by label: the picker holding the positions is gone.
                                    Some(crate::app::Picking::Asked { id, rows }) => {
                                        let chosen = rows
                                            .iter()
                                            .find(|(label, _)| *label == value)
                                            .map(|(_, choice)| choice.clone());
                                        match chosen {
                                            Some(choice) => UiCommand::Answered { id, choice },
                                            // No row matches, so answering would resume a tool with
                                            // a choice nobody made.
                                            None => continue,
                                        }
                                    }
                                    // Matched back by label, generated from these same scopes, so the
                                    // pairing is exact; a value matching none of them is the "no" row.
                                    Some(crate::app::Picking::Permission { id, offers }) => {
                                        let chosen = offers
                                            .iter()
                                            .find(|scope| {
                                                scope.label(&app.asking_about) == value
                                            });
                                        // The enforcing ledger is on the worker thread and never read
                                        // back, so what this session holds is kept here.
                                        if let Some(scope) = chosen
                                            && let Some(grant) = magi_tools::permit::standing(
                                                &app.asking_about,
                                                scope,
                                            )
                                        {
                                            app.was_granted(grant);
                                        }
                                        let decision = chosen.map_or(
                                            magi_proto::permit::Decision::Deny,
                                            |scope| magi_proto::permit::Decision::Allow {
                                                scope: scope.clone(),
                                                lifetime: magi_proto::permit::Lifetime::Session,
                                            },
                                        );
                                        UiCommand::Permit { id, decision }
                                    }
                                    // Taken above: its answer is not a `UiCommand`.
                                    Some(crate::app::Picking::Adoption { .. }) | None => continue,
                                };
                                direct(&mut app, &command_tx, command).await;
                                dirty = true;
                            }
                            // Leaving a question is an answer: closing one without a word left the
                            // session blocked, which on screen is a hang.
                            Action::Dismissed => {
                                match app.picking.take() {
                                    Some(crate::app::Picking::Permission { id, .. }) => {
                                        direct(
                                            &mut app,
                                            &command_tx,
                                            UiCommand::Permit {
                                                id,
                                                decision: magi_proto::permit::Decision::Deny,
                                            },
                                        )
                                        .await;
                                    }
                                    // Walking away is a no and has to be said: the asking session has
                                    // been waiting since its call came back.
                                    Some(crate::app::Picking::Adoption { id }) => {
                                        if let Some(layer) = layer.as_mut() {
                                            layer.answered(&id, false, None);
                                        }
                                    }
                                    _ => {}
                                }
                                dirty = true;
                            }
                            Action::ToggleDetail => {
                                // No notice: a view toggle did not happen in the conversation.
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
                            Action::Redraw | Action::Accepted | Action::Recalled | Action::Moved => dirty = true,
                            Action::Ignore => {}
                        }
                        // The popup is derived from the prompt, so it is recomputed after every key —
                        // except the key that just accepted one, and not while a list is open.
                        if !accepted
                            && !app
                                .overlay
                                .as_ref()
                                .is_some_and(magi_tui::overlay::Overlay::is_picker)
                        {
                            app.refresh_completion(&list_paths);
                        }
                    }
                    // magi asks for the pointer — see `terminal::MOUSE_ON` — so it selects text
                    // itself: mouse reporting is one terminal-wide switch.
                    Event::Mouse(mouse) => {
                        // A surface first, when the pointer landed on its rows. Not on a peer's: a
                        // pointer sent there would be driving that session.
                        if app.attached.is_none() && pointing::to_surface(&app, mouse, &command_tx).await {
                            continue;
                        }
                        let view = terminal_size().1.saturating_sub(ui::chrome_rows());
                        match pointing::on_the_screen(
                            &mut app,
                            mouse,
                            view,
                            terminal_size().0,
                            &mut copied,
                        ) {
                            pointing::Pointing::Redraw => dirty = true,
                            pointing::Pointing::Nothing => continue,
                        }
                    }
                    Event::Paste(text) => {
                        app.editor.insert_str(&text);
                        app.refresh_completion(&list_paths);
                        dirty = true;
                    }
                    Event::Resize(..) => {
                        // The width is the terminal's. Only the height is magi's to grant.
                        direct(
                            &mut app,
                            &command_tx,
                            UiCommand::Sized { rows: None, cols: inner(), holds: crate::terminal::reports_holds() },
                        )
                        .await;
                        dirty = true;
                    }
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                // The prompt's border scan runs whenever the box is on screen, and one that stopped
                // when a turn ended would read as the UI having frozen.
                app.advance();
                // Done on the frame rather than where the state changes: melchior and the socket both
                // answer with whatever they were last told.
                let mut ended = false;
                while let Ok(said) = heard.try_recv() {
                    match said {
                        // Handed to the session, not drawn here: an entry the UI appended for itself
                        // is one the model never sees. Kept while the screen is on a peer.
                        crate::melchior::Heard::Message { who, sort, text } => {
                            let arrived = app.received(&who, &sort, &text);
                            ours(&app, &command_tx, &mut held, arrived).await;
                        }
                        // Either shape. An older melchior says `names` and nothing else.
                        crate::melchior::Heard::Around { agents, names } => {
                            app.reachable = crate::melchior::peers(agents, names);
                        }
                        // The asking session is blocked on the answer, not on this turn.
                        crate::melchior::Heard::Asked { id, who, why } => {
                            app.asked_to_adopt(&id, &who, &why);
                        }
                        // Never into the transcript: permissions a model can read it can reason about.
                        crate::melchior::Heard::Adopted { by, handover } => {
                            let grants = handover
                                .as_deref()
                                .and_then(|said| serde_json::from_str(said).ok())
                                .unwrap_or_default();
                            ours(&app, &command_tx, &mut held, UiCommand::TakeGrants { grants })
                                .await;
                            app.notice_after_attach(format!(
                                "`{by}` took this session on. It may now do what that session may."
                            ));
                        }
                        crate::melchior::Heard::Stopped => ended = true,
                        // Said once, at startup, and read there.
                        crate::melchior::Heard::Listening { .. } => {}
                    }
                }
                if ended {
                    break;
                }
                // Counted rather than matched one for one: a sibling asking `status` wants to know
                // whether it is still waiting.
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

fn terminal_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// The socket to the session, and redialling one that dropped.
mod connecting;
use connecting::connection_loop;

/// Which agent the screen is pointed at, and what may be sent to one that is not ours.
mod crewing;
use crewing::{direct, footer_data, ours, walk};

/// The pointer, and which of two readers it belongs to.
mod pointing;

/// The colon commands. A closed list, in a file of its own.
mod commands;
use commands::{Control, run_command};

/// Handing the prompt to an editor, and what the screen says about itself.
#[path = "editing.rs"]
mod editing;
use editing::{debug_log, external_edit, inner};
