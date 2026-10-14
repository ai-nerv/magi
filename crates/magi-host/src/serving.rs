//! One connection to a session: who it is, what it may do, and every command it sends.

use super::*;

/// What every connection to one session shares.
#[derive(Clone)]
pub(super) struct Served {
    pub(super) session: Arc<Mutex<Session>>,
    pub(super) worker: Arc<tokio::sync::RwLock<Option<Arc<worker::Worker>>>>,
    pub(super) models: Arc<refreshing::Models>,
    pub(super) pending: Arc<crate::asking::Pending>,
    pub(super) person: crate::asking::Person,
    pub(super) scribe: crate::scribe::Held,
    /// What an `Attach` presents to control the session rather than watch it.
    pub(super) key: Arc<str>,
    /// How long a screen has to take one event before it is let go.
    pub(super) writing: std::time::Duration,
}

/// The key in `<socket>.key` beside the listener, minted when the binder did not write one.
pub(super) fn controller_key(listener: &UnixListener) -> Result<Arc<str>, HostError> {
    let address = listener.local_addr()?;
    let at = address.as_pathname().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a session needs a socket path",
        )
    })?;
    let key = match magi_ipc::read_key(at) {
        Some(key) => key,
        None => magi_ipc::mint_key(at)?,
    };
    Ok(Arc::from(key))
}

/// How long a connection has to say who it is.
const ATTACHING: std::time::Duration = std::time::Duration::from_secs(30);

/// How long an attached screen has to take one event.
pub(super) const WRITING: std::time::Duration = std::time::Duration::from_secs(30);

/// How many connections a session serves at once.
pub(super) const SEATS: usize = 64;

/// Turn away a connection past [`SEATS`], saying so, rather than dropping it unanswered.
pub(super) async fn busy(stream: UnixStream) {
    let mut writer = FrameWriter::new(stream);
    let refused = HarnessEvent::Refused {
        cursor: Cursor::ZERO,
        message: format!("busy: this session already serves {SEATS} connections"),
    };
    let _ = tokio::time::timeout(ATTACHING, writer.write(&refused)).await;
}

/// One attached UI. It controls the session when its `Attach` carries the key, and otherwise
/// watches: every command but leaving is refused.
pub(super) async fn connection(stream: UnixStream, served: Served) -> Result<(), HostError> {
    let Served {
        session,
        worker,
        models,
        pending,
        person,
        scribe,
        key: held,
        writing,
    } = served;
    let (worker, models, pending, person, scribe) =
        (&*worker, &models, &*pending, &person, &scribe);
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = crate::sending::Screen::within(FrameWriter::new(write_half), writing);

    let first = tokio::time::timeout(ATTACHING, reader.read::<UiCommand>())
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    let (from, draws, controls) = match first {
        UiCommand::Attach {
            from_cursor,
            draws,
            key,
            ..
        } => {
            let controls = key.is_some_and(|key| magi_ipc::same_key(&key, &held));
            (from_cursor, draws && controls, controls)
        }
        // Anything before an attach is a peer that does not speak the protocol.
        _ => return Ok(()),
    };
    // Whether anybody here can draw rows a tool asks for. Held for the life of the connection, so
    // a UI that goes away takes its screen with it.
    let _drawing = crate::holder::Drawing::attach(&person.surfaces, draws);

    // Subscribe before reading state, so an entry committed between the two is not lost in the gap.
    let (snapshot, backlog, mut live) = {
        let session = session.lock().await;
        (
            session.snapshot(from),
            session.replay(from),
            session.subscribe(),
        )
    };

    writer.write(&snapshot).await?;
    for event in backlog {
        writer.write(&event).await?;
    }
    // A question is an event and is in no journal, so the backlog cannot carry one. Whatever is
    // still waiting is asked again of the screen that has just arrived: one that stepped onto
    // another agent and came back would otherwise find a turn stuck on a prompt it never saw.
    for asked in pending.open() {
        writer.write(&asked).await?;
    }

    // Commands are read in their own task because `FrameReader::read` is not cancel-safe: it takes
    // a length then a body, and a `select!` dropping it between the two parses body as a length.
    // The channel is what says the client has gone: the sender is dropped when this task returns,
    // so a closed connection reaches the loop as the queue draining and then `None`.
    let (commands, mut incoming) = tokio::sync::mpsc::channel::<UiCommand>(32);
    let reading = ReaderTask(tokio::spawn(async move {
        while let Ok(command) = reader.read::<UiCommand>().await {
            if commands.send(command).await.is_err() {
                return;
            }
        }
    }));

    let mut refreshing = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = refreshing.join_next(), if !refreshing.is_empty() => {}
            command = incoming.recv() => {
                if let Some(asked) = &command {
                    magi_model::noted!("ui: {}", asked.verb());
                    if !controls && !asked.watches() {
                        let cursor = session.lock().await.cursor();
                        let message = format!(
                            "`{}` needs the session's controller key; this connection only watches",
                            asked.verb()
                        );
                        writer.write(&HarnessEvent::Refused { cursor, message }).await?;
                        continue;
                    }
                }
                match command {
                    Some(UiCommand::RefreshModels) => {
                        if refreshing.is_empty() {
                            let models = Arc::clone(models);
                            let session = Arc::clone(&session);
                            refreshing.spawn(async move { models.refresh(&session).await });
                        }
                    }
                    Some(command @ (UiCommand::SubmitPrompt { .. } | UiCommand::Arrived { .. }
                        | UiCommand::TakeGrants { .. } | UiCommand::DeclareNeeds)) => {
                        use session::admission::Request;
                        let request = match command {
                            UiCommand::SubmitPrompt { text, aside } => Request::Opening(Entry::User {
                                id: MessageId::new("queued"), text, aside,
                            }),
                            UiCommand::Arrived { who, kin, sort, text } =>
                                Request::Opening(Entry::From { who, kin, sort, text }),
                            UiCommand::TakeGrants { grants } => Request::Grants(grants),
                            UiCommand::DeclareNeeds => Request::Declare,
                            _ => unreachable!(),
                        };
                        let catalog = models.catalog.read().await.clone();
                        if let Err(message) = submit(&session, request, worker, &catalog, scribe).await {
                            let cursor = session.lock().await.cursor();
                            writer.write(&HarnessEvent::Refused {
                                cursor, message,
                            }).await?;
                        }
                    }
                    Some(command @ (UiCommand::SetModel { .. } | UiCommand::SetProvider { .. }
                        | UiCommand::SetThinking { .. })) => {
                        let catalog = models.catalog.read().await.clone();
                        let refusal = match command {
                            UiCommand::SetModel { name } =>
                                switch_model(&session, worker, &catalog, person, scribe, &name).await,
                            UiCommand::SetProvider { provider } =>
                                switch_provider(&session, worker, &catalog, person, scribe, provider).await,
                            UiCommand::SetThinking { level } =>
                                switch_thinking(&session, worker, &catalog, person, scribe, &level).await,
                            _ => None,
                        };
                        if let Some(message) = refusal {
                            let cursor = session.lock().await.cursor();
                            writer.write(&HarnessEvent::Refused { cursor, message }).await?;
                        }
                    }
                    Some(UiCommand::SetUnsure { band }) => {
                        judging::widen(&session, &person.standing, band).await;
                    }
                    Some(UiCommand::SetMode { mode }) => {
                        if let Err(message) = judging::switch(&session, &person.standing, &mode).await {
                            let cursor = session.lock().await.cursor();
                            writer.write(&HarnessEvent::Refused { cursor, message }).await?;
                        }
                    }
                    // To the screen that asked, not every screen: a view of notes is one person's.
                    Some(UiCommand::Memory { verb, arg }) => {
                        let _boundary = worker.read().await;
                        // The float's reads and the notes' are two shapes, not one with a flag.
                        if matches!(verb.as_str(), "recall" | "sessions" | "why") {
                            writer
                                .write(&crate::knowing::held(scribe, &verb, &arg).await)
                                .await?;
                        } else {
                            for answer in crate::knowing::notes(scribe, &verb, arg).await {
                                writer.write(&answer).await?;
                            }
                        }
                    }
                    Some(UiCommand::Resume { id }) => {
                        if let Err(message) = resuming::resume(&session, worker, scribe, &id).await {
                            let cursor = session.lock().await.cursor();
                            writer
                                .write(&HarnessEvent::Refused {
                                    cursor,
                                    message,
                                })
                                .await?;
                        }
                    }
                    Some(UiCommand::Branch { keeps }) => {
                        let mut held = session.lock().await;
                        if held.busy() {
                            let cursor = held.cursor();
                            drop(held);
                            writer.write(&HarnessEvent::Refused {
                                cursor,
                                message: "session is busy; branch after queued work finishes".into(),
                            }).await?;
                            continue;
                        }
                        if let Some(keeps) =
                            keeps.or_else(|| context::rewind_point(held.entries()))
                        {
                            // Journalled, not applied: the entries it skips are still there.
                            let id = MessageId::new(format!("b{}", held.cursor().next().0));
                            held.commit(Entry::Branch { id, keeps })?;
                        }
                    }
                    // Handed straight to whoever is blocked on it; an id nobody waits on is dropped.
                    Some(UiCommand::Permit { id, decision }) => {
                        pending.answer(&id, decision);
                    }
                    // The same, for a question a tool asked in its own words.
                    Some(UiCommand::Answered { id, choice }) => {
                        pending.chose(&id, choice);
                    }
                    // How much room the screen has. The session has no terminal of its own.
                    Some(UiCommand::Sized { rows, cols, holds }) => {
                        person.surfaces.sized(rows, cols, holds);
                    }
                    // And the float's inside, which a surface asking for the float is given whole.
                    Some(UiCommand::FloatSized { rows, cols }) => {
                        person.surfaces.floated(rows, cols);
                    }
                    // A key aimed at rows a tool is holding, not interpreted on the way through.
                    Some(UiCommand::Keyed { id, key, state }) => {
                        person.surfaces.keyed(&id, key, state);
                    }
                    // The pointer, already in the surface's own coordinates: the UI translated it.
                    Some(UiCommand::Moused { id, kind, button, row, col }) => {
                        person.surfaces.moused(&id, kind, button, row, col);
                    }
                    Some(UiCommand::Unsurface { id }) => person.surfaces.close(&id),
                    Some(UiCommand::Interrupt) => {
                        let held = session.lock().await;
                        held.cancel().request();
                        person.surfaces.close_all();
                    }
                    Some(UiCommand::Attach { .. }) => {}
                    Some(UiCommand::Detach) | None => break,
                }
            }
            event = live.recv() => {
                match event {
                    // Awaited in the branch body, not as a select arm: a cancelled write desyncs.
                    Ok(event) => writer.write(&event).await?,
                    // A screen that fell behind has lost deltas it cannot ask for again, so it is
                    // handed the whole session as it now stands.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let now = {
                            let held = session.lock().await;
                            held.snapshot(held.cursor())
                        };
                        writer.write(&now).await?;
                    }
                    Err(_) => break,
                }
            }
        }
    }

    drop(reading);
    Ok(())
}

#[cfg(test)]
mod tests;

struct ReaderTask(tokio::task::JoinHandle<()>);

impl Drop for ReaderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
