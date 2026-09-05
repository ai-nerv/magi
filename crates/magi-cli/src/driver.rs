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
use crossterm::event::{Event, EventStream};
use magi_proto::{HarnessEvent, UiCommand};
use magi_tui::footer::FooterData;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

/// How long to wait before redialling a session that went away.
const RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// Run the UI until the user quits.
///
/// `prompt` is the positional argument: `magi "…"` opens the UI with the question already
/// asked, so the first thing on screen is the answer arriving rather than an empty box the
/// user has to retype into.
pub async fn run(
    socket: &Path,
    prompt: Option<String>,
    loaded: Option<crate::config::Loaded>,
    project: &str,
    started: Option<(crate::melchior::Melchior, std::path::PathBuf)>,
) -> Result<()> {
    // **Before anything reads a setting.** `colour`, `glyph` and `metric` each hold their table
    // in a `OnceLock` that the first *read* fills with the built-in defaults, and `adopt` after
    // that is a no-op. `App::new` reads one — it needs a line for the empty prompt — so building
    // it first threw the whole configured `magi.ui` away, silently, and left the box repeating
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
    // What the configuration already allows, so a session that takes a child on can lend it what
    // it holds from the first moment rather than only what a person has answered a prompt with
    // since. The ledger the session enforces with is seeded from the same list.
    if let Some(loaded) = &loaded {
        app.granted = crate::config::granted(loaded);
    }
    // What the footer shows. melchior names a session because it can see the namespace and magi
    // cannot; without melchior there are no siblings to be told apart, so the project is name enough.
    app.named = started
        .as_ref()
        .map_or_else(|| project.to_owned(), |(layer, _)| layer.named.clone());
    // The prompts from previous runs, so the arrow keys reach past this one.
    app.editor = magi_tui::Editor::with_history(crate::history::load());
    if let Some(loaded) = &loaded {
        // Worked out here because the answer needs the catalog, and the snapshot carries only
        // whether there is a model — not why there is not. Same text the session refuses a prompt
        // with, so meeting the problem at attach and meeting it at the first prompt say one thing.
        // The cards come from melchior, so this asks it. One process at attach, against the
        // alternative of a picker that lists what magi believed rather than what can be reached.
        let catalog = crate::config::catalog(loaded, magi_host::broker::cards().await);
        if crate::config::backend(&catalog).is_none() {
            app.no_model = Some(magi_host::no_model(&catalog));
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
            "This session started before {} changed. Quit and start magi again to pick it up.",
            names.join(", ")
        ));
    }
    let mut session = Session::open()?;
    // From here, not from the start of `main`: the clock is for the screen, and there is no
    // screen until the alternate one is open.
    magi_tui::decrypt::begin();
    let mut terminal_events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(magi_tui::metric::frame_ms()));

    let (event_tx, mut event_rx) = mpsc::channel::<HarnessEvent>(256);
    let (command_tx, command_rx) = mpsc::channel::<UiCommand>(32);
    // Shared rather than inferred from the event stream: a dropped connection produces no
    // event, so a UI watching only for events cannot tell "nothing is happening" from
    // "nothing can happen".
    let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // What melchior says, on its way to becoming an entry. Read on a thread because it is a
    // blocking pipe and everything else in this loop is not — and dropped, along with melchior
    // itself, when this function returns.
    let (heard_tx, mut heard) = mpsc::channel::<crate::melchior::Heard>(64);
    let mut layer = started.map(|(layer, _at)| layer);
    if let Some(reading) = layer.as_mut().and_then(crate::melchior::Melchior::hearing) {
        std::thread::spawn(move || {
            use std::io::BufRead;
            // Already buffered, and handed over as the reader for that reason: the line that
            // named this session was read through it, and whatever followed the newline is
            // sitting in it. Wrapping the raw pipe again here would have thrown that away.
            for line in reading.lines().map_while(Result::ok) {
                // A line this build cannot read is a newer melchior, not a reason to stop reading:
                // the next one may well be a message, and dropping the whole pipe over a field
                // nobody here knows about would lose it.
                let Ok(said) = serde_json::from_str::<crate::melchior::Heard>(&line) else {
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
    // Whether the extra keyboard layer is up. A surface reading a *hold* needs releases on keys
    // that produce text, and asking for those globally stops `:` opening the command line — so the
    // layer goes on when a surface takes the keyboard and comes off when it gives it back.
    let mut keys_held = false;
    // Whether the last key a surface was sent was an escape. Two in a row take the screen back,
    // so `esc` itself stays a key the tenant can read -- see where a surface is handed one.
    let mut escaped = false;
    // What shape the terminal was last told to draw its cursor in. Insert mode is a bar and
    // normal mode a block, which is the one cue that says which mode you are in without
    // looking away from what you are typing.
    let mut shown = magi_tui::vim::Mode::Insert;
    // Set by a mouse release, acted on after the next draw: the text a selection covers is read
    // back out of the frame it was drawn into, so there has to be a frame.
    let mut copied: Option<magi_tui::select::Selection> = None;
    // Whether a turn was running last frame. A turn *ending* is the edge that answers an
    // arrival, and neither side of the pipe can see it: melchior cannot see a turn at all, and the
    // session publishes what it is doing rather than what it just stopped doing.
    let mut was_busy = false;
    // The room a surface would have had in the last frame drawn. The session grants rows out of
    // this, and it moves without the window doing: a prompt that wrapped onto a second line took
    // a row off it. Compared rather than sent every frame, so a redraw per keystroke is not also
    // a command per keystroke.
    let mut told_room = None;
    loop {
        // Read each pass rather than tracked here: the connection lives in another task, and
        // this is the one thing about it the screen has to show.
        let attached_now = attached.load(Ordering::Relaxed);
        if attached_now != app.connected {
            app.connected = attached_now;
            dirty = true;
        }
        // Compared each pass rather than hooked onto the events, because a surface can end several
        // ways — answered, cancelled, its tenant gone — and every one of them has to give the
        // keyboard back. What matters is whether one is holding it now.
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
            // Measured in the draw, told after it. The session has no terminal and cannot work
            // this out, so a tool asking for eight rows on a window with three would be granted
            // eight and lay itself out for five nobody can see.
            if told_room != Some(room) {
                told_room = Some(room);
                let _ = command_tx
                    .send(UiCommand::Sized {
                        rows: Some(room),
                        cols: inner(),
                        holds: crate::terminal::reports_holds(),
                    })
                    .await;
            }
            // Read out of the frame that was just drawn, which is what `draw` hands back.
            //
            // **Not `current_buffer_mut`.** ratatui keeps two buffers and ends every draw with
            // `swap_buffers`, which *resets* the one it is about to make current — so the
            // "current" buffer after a draw is blank, and the text taken from it was always the
            // empty string. Nothing ever reached the clipboard: the highlight appeared, the
            // release was seen, and the copy silently did nothing.
            let copy = copied
                .take()
                .map(|sel| magi_tui::select::text(drawn.buffer, sel, drawn.area))
                .filter(|text| !text.is_empty());
            dirty = false;
            // After the frame, and only when it has changed: the shape is the terminal's own
            // cursor, so it outlives a redraw and does not need setting on every one.
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
                    // **Every kind of key event, not only presses.** With the Kitty protocol a
                    // held key arrives as `Repeat` and a released one as `Release`, and both are
                    // needed: a surface cannot tell "tapped" from "still holding" without them.
                    // Which of the three each reader wants is decided below rather than here.
                    Event::Key(key) => {
                        // Somebody is here. Whatever the box was writing to itself, it stops and
                        // starts its wait over -- including on the keys that leave the prompt
                        // empty, which is most of them at this point.
                        app.tease.interrupt();
                        // **This is how magi learns the protocol is live.** No terminal sends a
                        // repeat or a release unless it is, so the first one is proof where the
                        // startup probe was only a guess — and a surface already on screen is told,
                        // once, so it can offer the control it now knows it has.
                        if matches!(
                            key.kind,
                            crossterm::event::KeyEventKind::Repeat
                                | crossterm::event::KeyEventKind::Release
                        ) && crate::terminal::noticed_hold()
                        {
                            let _ = command_tx
                                .send(UiCommand::Sized {
                                    rows: None,
                                    cols: inner(),
                                    holds: true,
                                })
                                .await;
                        }
                        // **A surface has the keyboard while it has the rows.** Forwarded by
                        // name and not interpreted: what `j` means is the tenant's business, and
                        // a driver that decided would be back to owning what it just handed over.
                        //
                        // **Escape twice takes the screen back.** It used to be once, which is
                        // how a person gets out of a tenant that has stopped answering — and then
                        // a tenant turned out to be able to hold a pty, where `esc` is a key the
                        // program inside wants and closes nothing. So a single one is forwarded
                        // like any other, and only a second with nothing between them is magi's.
                        // Every existing tenant answers the first one anyway, so the second never
                        // arrives; a program in a pty gets both.
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
                                // **What the terminal actually sent.** The one question nobody
                                // can answer by looking at the screen: a list stepping two rows
                                // for one press is a release being read as a press, or the same
                                // press arriving twice, and those are bugs in different programs.
                                // One line per event settles which — see `debug_log`.
                                debug_log(format_args!(
                                    "surface key {named} {:?}",
                                    crate::keying::held(key)
                                ));
                                let _ = command_tx
                                    .send(UiCommand::Keyed {
                                        id,
                                        key: named,
                                        state: crate::keying::held(key),
                                    })
                                    .await;
                            }
                            continue;
                        }
                        // The prompt is text, and text has no use for a release. A *repeat* it
                        // very much has: with the protocol on, holding backspace arrives as
                        // repeats, and a reader that took only presses would delete one character
                        // and then stop — on exactly the terminals that support this.
                        if key.kind == crossterm::event::KeyEventKind::Release {
                            continue;
                        }
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
                                // and that there is a tool for reaching it -- magi delivering
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
                                // Answered down the pipe, not over the socket, so it is taken
                                // before the list that turns a choice into a `UiCommand`. melchior
                                // is holding a request another session is blocked on; this
                                // session's own turn loop knows nothing about it.
                                if let Some(crate::app::Picking::Adoption { id }) =
                                    app.picking.as_ref()
                                {
                                    let (id, accept) = (id.clone(), value == "yes");
                                    // Taken at the moment of accepting, and it does not track
                                    // afterwards: what was consented to is what was on the table
                                    // when the question was answered.
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
                                    // Matched back by label, because that is what the person
                                    // read and chose, and the picker that held the positions is
                                    // already gone by here. The tool asked to get an id back,
                                    // which is not what a row says.
                                    Some(crate::app::Picking::Asked { id, rows }) => {
                                        let chosen = rows
                                            .iter()
                                            .find(|(label, _)| *label == value)
                                            .map(|(_, choice)| choice.clone());
                                        match chosen {
                                            Some(choice) => UiCommand::Answered { id, choice },
                                            // Nothing to send: no row matches, so answering
                                            // would resume a tool with a choice nobody made.
                                            None => continue,
                                        }
                                    }
                                    // Matched back by *label*, because that is what the person
                                    // read and chose. The labels were generated from these same
                                    // scopes a moment ago, so the pairing is exact rather than
                                    // a guess — and a value matching none of them is the "no"
                                    // row, which is the only other thing in the list.
                                    Some(crate::app::Picking::Permission { id, offers }) => {
                                        let chosen = offers
                                            .iter()
                                            .find(|scope| {
                                                scope.label(&app.asking_about) == value
                                            });
                                        // Remembered here because here is where it is known. The
                                        // ledger that enforces it lives on the worker thread and
                                        // is never read back, and a session that lends its
                                        // permissions to a child has to know what it holds.
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
                                    // Taken above, before this list: its answer is not a
                                    // `UiCommand` and has nowhere to go from here.
                                    Some(crate::app::Picking::Adoption { .. }) | None => continue,
                                };
                                let _ = command_tx.send(command).await;
                                dirty = true;
                            }
                            // Leaving a question is an answer to it. A permission prompt is the
                            // only list something is waiting on, and the wait is a turn that
                            // has stopped: closing it without a word left the session blocked
                            // until its own patience ran out, which on screen is a hang.
                            Action::Dismissed => {
                                match app.picking.take() {
                                    Some(crate::app::Picking::Permission { id, .. }) => {
                                        let _ = command_tx
                                            .send(UiCommand::Permit {
                                                id,
                                                decision: magi_proto::permit::Decision::Deny,
                                            })
                                            .await;
                                    }
                                    // Walking away is a no, and it has to be *said*. The asking
                                    // session has been waiting since its call came back with
                                    // "the question has been put"; a dismissal that answered
                                    // nothing would leave it waiting for good, and the person
                                    // who closed the box would think they had refused.
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
                                .is_some_and(magi_tui::overlay::Overlay::is_picker)
                        {
                            app.refresh_completion(&list_paths);
                        }
                    }
                    // magi asks for the pointer -- see `terminal::MOUSE_ON` -- which is why it
                    // does its own text selection: mouse reporting is one terminal-wide switch,
                    // and an application holding it stops the terminal running its own drag.
                    Event::Mouse(mouse) => {
                        // A surface first, when the pointer landed on the rows one is holding.
                        // Everything else on the screen is magi's -- see `driver::pointing`.
                        if pointing::to_surface(&app, mouse, &command_tx).await {
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
                        // The width is the terminal's and changes under whatever is drawing in
                        // the rows a tool was given. Only the height is magi's to grant.
                        let _ = command_tx
                            .send(UiCommand::Sized { rows: None, cols: inner(), holds: crate::terminal::reports_holds() })
                            .await;
                        dirty = true;
                    }
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
                // What melchior heard, and what it is allowed to say about us. Done on the frame
                // rather than where the state changes: melchior answers with whatever it was last
                // told, and a session that only told it on some paths would report a turn that
                // finished a minute ago.
                let mut ended = false;
                while let Ok(said) = heard.try_recv() {
                    match said {
                        // Handed to the session, not drawn here. melchior is where a message lands,
                        // but the transcript and the turns are the session's — an entry the UI
                        // appended for itself is one the model never sees, and an instance
                        // could be asked a question and sit there until somebody typed at it.
                        crate::melchior::Heard::Message { who, sort, text } => {
                            let _ = command_tx.send(app.received(&who, &sort, &text)).await;
                        }
                        crate::melchior::Heard::Around { names } => app.reachable = names,
                        // Straight to the screen, and it takes it. Nothing is blocked on this
                        // turn — the asking session is blocked on the *answer*, and it has been
                        // told to expect one.
                        crate::melchior::Heard::Asked { id, who, why } => {
                            app.asked_to_adopt(&id, &who, &why);
                        }
                        // Straight into the ledger, never into the transcript. Permissions a
                        // model can read are permissions it can reason about acquiring more of.
                        crate::melchior::Heard::Adopted { by, handover } => {
                            let grants = handover
                                .as_deref()
                                .and_then(|said| serde_json::from_str(said).ok())
                                .unwrap_or_default();
                            let _ = command_tx
                                .send(UiCommand::TakeGrants { grants })
                                .await;
                            app.notice_after_attach(format!(
                                "`{by}` took this session on. It may now do what that session may."
                            ));
                        }
                        crate::melchior::Heard::Stopped => ended = true,
                        // Said once, at startup, and read there. A second one is a newer melchior
                        // saying something this build has no use for.
                        crate::melchior::Heard::Listening { .. } => {}
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

/// The socket to the session, and redialling one that dropped.
mod connecting;
use connecting::connection_loop;

/// The pointer, and which of two readers it belongs to.
mod pointing;

/// The colon commands. A closed list, in a file of its own.
mod commands;
use commands::{Control, run_command};

/// Handing the prompt to an editor, and what the screen says about itself.
#[path = "editing.rs"]
mod editing;
use editing::{debug_log, external_edit, inner};
