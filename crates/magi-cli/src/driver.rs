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
use std::time::{Duration, Instant};
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
    attach: Option<String>,
    view_only: bool,
) -> Result<()> {
    // Before anything reads a setting: `colour`, `glyph` and `metric` each hold their table in a
    // `OnceLock` the first read fills with defaults, and `adopt` after that is a no-op.
    if let Some(loaded) = &loaded {
        crate::config::adopt_ui(loaded);
    }
    let mut app = App::new();
    // A `--attach <id>` waits for that agent to show up in the roster, then points the screen at it.
    app.attach_wanted = attach;
    if let Some(loaded) = &loaded {
        app.about = crate::config::agents::descriptions(loaded);
        app.mind = crate::config::mind(loaded);
    }
    app.view_only = view_only;
    // Whether casper answers is asked once, off the UI thread: the probe starts the program.
    let casper_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(loaded) = &loaded {
        let program = crate::config::tooling(loaded).program;
        let up = Arc::clone(&casper_up);
        tokio::task::spawn_blocking(move || {
            up.store(
                !magi_tools::supplier::cards_from(&program).is_empty(),
                Ordering::Relaxed,
            );
        });
    }
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
    // The latest child/watched edge worth a turn, held until this session is idle and its own
    // screen is up — then run as a turn so the coordinator reacts, the wake a headless `park` runs.
    let mut pending_wake: Option<String> = None;
    let mut last_wake: Option<Instant> = None;
    // A lead's own prompts carry the size check, so whether to coordinate is decided at the task.
    let seat = crate::config::seat();

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
                aside: seat.remind(String::new()),
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
    let mut told_float: Option<(u16, u16)> = None;
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
            // And the float's inside, which a surface asking for the float is given whole.
            let (rows, cols) = float_room();
            if told_float != Some((rows, cols)) {
                told_float = Some((rows, cols));
                direct(&mut app, &command_tx, UiCommand::FloatSized { rows, cols }).await;
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
                            // magi's own, never the tenant's: ctrl+c ends it, ctrl+d goes nowhere.
                            let ctrl = key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL);
                            if ctrl && matches!(key.code, crossterm::event::KeyCode::Char('c' | 'd'))
                            {
                                if key.code == crossterm::event::KeyCode::Char('c')
                                    && key.kind == crossterm::event::KeyEventKind::Press
                                {
                                    app.surface = None;
                                    escaped = false;
                                    direct(&mut app, &command_tx, UiCommand::Unsurface { id })
                                        .await;
                                }
                                continue;
                            }
                            if key.kind != crossterm::event::KeyEventKind::Release {
                                if key.code == crossterm::event::KeyCode::Esc {
                                    // Ended at the session too, or its program runs on unseen.
                                    if escaped {
                                        app.surface = None;
                                        escaped = false;
                                        direct(&mut app, &command_tx, UiCommand::Unsurface { id })
                                            .await;
                                        continue;
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
                            Action::Forget(value) => {
                                app.forget_row(&value);
                                dirty = true;
                            }
                            Action::Submit(text) if app.view_only => {
                                app.refuse_view_only();
                                app.editor.insert_str(&text);
                                dirty = true;
                            }
                            Action::Submit(text) => {
                                crate::history::remember(&text);
                                // Beside the prompt, not appended to it: naming an instance tells the
                                // model it is there and that a tool reaches it.
                                let briefed = layer
                                    .as_ref()
                                    .map_or_else(String::new, |l| l.briefing(&text, project));
                                let aside = if app.attached.is_none() {
                                    seat.remind(briefed)
                                } else {
                                    briefed
                                };
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
                            Action::Tab { forward } => {
                                floating::step(&mut app, &command_tx, forward).await;
                                dirty = true;
                            }
                            Action::Attach(id) if app.pane_titled("balthasar") => {
                                floating::weigh(&mut app, &command_tx, &id).await;
                                dirty = true;
                            }
                            // On the model's card, Enter takes a setting and ←/→ step it.
                            Action::Attach(id) if app.chooses() => {
                                if let Some(command) = app.choose_on_pane(&id) {
                                    direct(&mut app, &command_tx, command).await;
                                }
                                dirty = true;
                            }
                            Action::Fold { open }
                                if app.pane_titled("model") || app.pane_titled("permission") =>
                            {
                                floating::fold(&mut app, &command_tx, open).await;
                                dirty = true;
                            }
                            // Enter on an entry in the agents view: the same as a click on it.
                            Action::Attach(id) => {
                                if let Some(seat) = app.attach_id(&id) {
                                    dial(&app, seat, socket, &target_tx, &command_tx, &mut held)
                                        .await;
                                }
                                dirty = true;
                            }
                            Action::Fold { open } => {
                                app.fold_agent(open);
                                dirty = true;
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
                                let Some(command) = app.chose(value) else {
                                    dirty = true;
                                    continue;
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
                        let pointed =
                            pointing::on_the_screen(&mut app, mouse, view, terminal_size().0, &mut copied);
                        if matches!(pointed, pointing::Pointing::Nothing) {
                            continue;
                        }
                        // A row in the agents view dials that agent, the way `walk` does for the
                        // keys; a tab sends what its tab needs asked.
                        if let pointing::Pointing::Steer(seat) = pointed {
                            dial(&app, seat, socket, &target_tx, &command_tx, &mut held).await;
                        } else if let pointing::Pointing::Ask(command) = pointed {
                            let _ = command_tx.send(command).await;
                        }
                        dirty = true;
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
                app.poll_details();
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
                            // An `attention` or `trouble` message is the one kind meant to reach a
                            // session mid-turn: interrupt this one's own turn so it attends sooner.
                            // Only while our own session is on screen: attached, the interrupt would
                            // stop the agent we are driving instead.
                            if app.attached.is_none()
                                && app.is_busy()
                                && crate::melchior::interrupts(&sort)
                            {
                                direct(&mut app, &command_tx, UiCommand::Interrupt).await;
                            }
                        }
                        // Either shape. An older melchior says `names` and nothing else.
                        crate::melchior::Heard::Around { agents, names } => {
                            app.reachable = crate::melchior::peers(agents, names);
                            // A `--attach <id>` points the screen at its target once that agent
                            // appears on the roster with a screen to draw over, as a manual
                            // `alt+.` onto it would.
                            if let Some(id) = app.attach_wanted.clone()
                                && let Some(them) = app
                                    .reachable
                                    .iter()
                                    .find(|them| them.id == id && them.ui.is_some())
                                    .cloned()
                                && let Some(at) = them.ui.clone()
                            {
                                app.attach_to(Some(them));
                                let _ = target_tx.send(at);
                                app.attach_wanted = None;
                            }
                        }
                        // A watched agent changed phase. A dim line so a coordinator sees a child
                        // finish or hit trouble, and — for the edges that end a wait — an occasion
                        // queued to wake this session once it is idle, the same turn a headless
                        // `park` would run. The rest updates the panel silently.
                        crate::melchior::Heard::Signal {
                            from,
                            kind,
                            kin,
                            cause,
                        } => {
                            if app.attached.is_none()
                                && let Some(note) = signal_notice(&from, &kind, cause.as_deref())
                            {
                                app.show_notice(note);
                            }
                            if let Some(occasion) =
                                crate::child::wake_prompt(&kin, &kind, &from, cause.as_deref())
                            {
                                pending_wake = Some(occasion);
                            }
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
                app.siblings = [
                    layer
                        .as_mut()
                        .is_some_and(crate::melchior::Melchior::alive),
                    crate::balthasar::alive(),
                    casper_up.load(Ordering::Relaxed),
                ];
                if let Some(layer) = layer.as_mut() {
                    let (phase, cause) = app.phase();
                    layer.doing(
                        app.is_busy(),
                        app.elapsed().map_or(0, |since| since.as_secs()),
                        Some(app.unanswered()),
                        phase,
                        cause.as_deref(),
                        &app.spent(),
                    );
                }
                // Take it up only into an idle session on its own screen, with no half-typed prompt
                // and no question waiting, no faster than the cooldown — so a burst lands as one
                // turn and never steps on the person at the keyboard.
                let cooled = last_wake.is_none_or(|at| at.elapsed() >= crate::child::WAKE_COOLDOWN);
                if pending_wake.is_some()
                    && app.attached.is_none()
                    && !app.is_busy()
                    && !app.questioned()
                    && app.editor.is_blank()
                    && cooled
                    && let Some(occasion) = pending_wake.take()
                {
                    last_wake = Some(Instant::now());
                    let aside = layer
                        .as_ref()
                        .map_or_else(String::new, |l| l.briefing(&occasion, project));
                    direct(
                        &mut app,
                        &command_tx,
                        UiCommand::SubmitPrompt { text: occasion, aside },
                    )
                    .await;
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

/// The float's inside at this terminal size, as `(rows, cols)`: all a float surface is given.
pub(super) fn float_room() -> (u16, u16) {
    let (width, height) = terminal_size();
    let inside = magi_tui::pane::Pane::surface_inside(magi_tui::pane::Pane::area(
        ratatui::layout::Rect::new(0, 0, width, height),
    ));
    (inside.height, inside.width)
}

/// The socket to the session, and redialling one that dropped.
mod connecting;
use connecting::connection_loop;

/// Which agent the screen is pointed at, and what may be sent to one that is not ours.
mod crewing;
use crewing::{dial, direct, footer_data, ours, signal_notice, walk};

/// The pointer, and which of two readers it belongs to.
mod floating;
mod pointing;

/// The colon commands. A closed list, in a file of its own.
mod commands;
use commands::{Control, run_command};

/// Handing the prompt to an editor, and what the screen says about itself.
#[path = "editing.rs"]
mod editing;
use editing::{debug_log, external_edit, inner};
