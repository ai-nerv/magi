use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Seek, Write},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

use agent_client_protocol_schema::v1::{
    CancelNotification, Error, LoadSessionResponse, NewSessionResponse, RequestId,
};
use anyhow::{Context, Result, bail};
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{HarnessEvent, UiCommand};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWrite},
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    process::Child,
    sync::{mpsc, watch},
    task::JoinSet,
};

use super::{
    bootstrap::{self, Bootstrap},
    envelope, events, history, intake, permissions, prompt, settings, transport, turns,
};

const STARTUP: Duration = Duration::from_secs(30);
const SHUTDOWN: Duration = Duration::from_secs(5);

pub(super) struct Sessions {
    pub display: super::display::Policy,
    owned: BTreeMap<String, Session>,
    opening: BTreeSet<u64>,
    replaying: mpsc::Sender<history::Delivery>,
    pub replayed: mpsc::Receiver<history::Delivery>,
    generation: u64,
    permissions: permissions::Permissions,
    events: mpsc::Sender<events::Event>,
    pub incoming: mpsc::Receiver<events::Event>,
    failure: watch::Sender<bool>,
    pub failed: watch::Receiver<bool>,
    closing: watch::Sender<bool>,
    pub pending: JoinSet<Opened>,
}

pub(super) struct Opened {
    generation: u64,
    loading: bool,
    request: RequestId,
    _lease: intake::Item,
    session: Result<Session>,
}

struct Session {
    child: Child,
    _directory: Directory,
    identity: events::Identity,
    reader: events::Reader,
    writer: FrameWriter<OwnedWriteHalf>,
    turns: turns::Stream,
    settings: settings::Settings,
    _mcp: Bootstrap,
}

struct Attached {
    identity: events::Identity,
    reader: OwnedReadHalf,
    writer: FrameWriter<OwnedWriteHalf>,
    settings: settings::Settings,
}

struct Directory(PathBuf);

pub(super) enum Opening {
    New(Value),
    Load(Value),
}

impl Default for Sessions {
    fn default() -> Self {
        let (events, incoming) = mpsc::channel(4);
        let (failure, failed) = watch::channel(false);
        let (closing, _) = watch::channel(false);
        let (replaying, replayed) = mpsc::channel(1);
        Self {
            display: super::display::Policy::default(),
            owned: BTreeMap::new(),
            opening: BTreeSet::new(),
            replaying,
            replayed,
            generation: 0,
            permissions: permissions::Permissions::default(),
            events,
            incoming,
            failure,
            failed,
            closing,
            pending: JoinSet::new(),
        }
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        for name in ["bootstrap.json", "host", "host.key", "host.lock"] {
            let _ = std::fs::remove_file(self.0.join(name));
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

impl Sessions {
    pub(super) async fn create(
        &mut self,
        request: RequestId,
        params: Opening,
        mut item: intake::Item,
        closed: watch::Receiver<bool>,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let loading = matches!(&params, Opening::Load(_));
        let checked = (|| {
            if self.owned.len() + self.pending.len() >= 4 {
                return Err(Error::new(-32000, "Session limit reached"));
            }
            let (cwd, bootstrap) = match params {
                Opening::New(params) => bootstrap::validate(params)
                    .map(|(requested, bootstrap)| (requested.cwd, bootstrap)),
                Opening::Load(params) => bootstrap::validate_load(params),
            }
            .map_err(|_| Error::invalid_params())?;
            if let Some(native) = bootstrap.loading()
                && self
                    .owned
                    .contains_key(&history::external(native).map_err(|_| Error::invalid_params())?)
            {
                return Err(Error::new(-32000, "Native session is already open"));
            }
            if *closed.borrow() {
                return Err(Error::new(-32000, "Client disconnected"));
            }
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| Error::new(-32000, "Session generation exhausted"))?;
            Ok((cwd, bootstrap, self.generation))
        })();
        match checked {
            Ok((cwd, bootstrap, generation)) => {
                let events = self.events.clone();
                let failure = self.failure.clone();
                let closing = self.closing.subscribe();
                let replay = history::Replay {
                    generation,
                    policy: self.display,
                    output: self.replaying.clone(),
                };
                self.opening.insert(generation);
                self.pending.spawn(async move {
                    let session =
                        start(cwd, bootstrap, replay, closed, closing, events, failure).await;
                    Opened {
                        generation,
                        loading,
                        request,
                        _lease: item,
                        session,
                    }
                });
            }
            Err(error) => {
                transport::reply(output, &envelope::failure(Some(request), error), &mut item)
                    .await?
            }
        }
        Ok(())
    }

    pub(super) async fn opened(
        &mut self,
        mut opened: Opened,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        self.opening.remove(&opened.generation);
        let mut activate = None;
        let response = match opened.session {
            Ok(mut session) if self.owned.contains_key(&session.identity.external) => {
                session.reader.finish().await;
                stop(&mut session.child).await;
                envelope::failure(
                    Some(opened.request),
                    Error::new(-32000, "Native session is already open"),
                )
            }
            Ok(session) => {
                let id = session.identity.external.clone();
                let result = if opened.loading {
                    serde_json::to_value(
                        LoadSessionResponse::new().config_options(session.settings.options()),
                    )?
                } else {
                    serde_json::to_value(
                        NewSessionResponse::new(id.clone())
                            .config_options(session.settings.options()),
                    )?
                };
                activate = Some(id.clone());
                self.owned.insert(id, session);
                json!({"jsonrpc":"2.0", "id":opened.request, "result":result})
            }
            Err(why) => {
                eprintln!("magi acp: native startup: {why:#}");
                envelope::failure(
                    Some(opened.request),
                    Error::new(-32000, "Native session startup failed"),
                )
            }
        };
        transport::reply(output, &response, &mut opened._lease).await?;
        if let Some(id) = activate
            && let Some(session) = self.owned.get_mut(&id)
        {
            session.reader.activate();
        }
        drop(opened._lease);
        Ok(())
    }

    pub(super) async fn replay(
        &mut self,
        replay: history::Delivery,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        if self.opening.contains(&replay.generation) {
            replay.write(output).await?;
        }
        Ok(())
    }

    pub(super) async fn prompt(
        &mut self,
        id: RequestId,
        params: Value,
        mut item: intake::Item,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let admitted = prompt::parse(params).and_then(|prompt| {
            let session = self
                .owned
                .get_mut(&prompt.session)
                .ok_or_else(|| Error::new(-32000, "Unknown session"))?;
            if session.turns.busy() || session.settings.busy() {
                return Err(Error::new(-32000, "Session already has a pending prompt"));
            }
            Ok((session, prompt.command))
        });
        match admitted {
            Ok((session, command)) => {
                let request = session.turns.submit(id, item)?;
                let UiCommand::SubmitPrompt {
                    text,
                    aside,
                    images,
                } = command
                else {
                    bail!("invalid parsed prompt command");
                };
                let command = UiCommand::SubmitTracked {
                    request,
                    text,
                    aside,
                    images,
                };
                tokio::time::timeout(transport::DEADLINE, session.writer.write(&command)).await??;
            }
            Err(error) => {
                transport::reply(output, &envelope::failure(Some(id), error), &mut item).await?
            }
        }
        Ok(())
    }

    pub(super) async fn configure(
        &mut self,
        id: RequestId,
        params: Value,
        item: intake::Item,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let mut item = Some(item);
        let admitted = settings::parse(params).and_then(|(key, model)| {
            let session = self
                .owned
                .get_mut(&key)
                .ok_or_else(|| Error::new(-32000, "Unknown session"))?;
            if session.turns.busy() {
                return Err(Error::new(-32000, "Session has an active prompt"));
            }
            let command = session.settings.select(id.clone(), model, &mut item)?;
            Ok((session, command))
        });
        match admitted {
            Ok((session, command)) => {
                tokio::time::timeout(transport::DEADLINE, session.writer.write(&command)).await??;
            }
            Err(error) => {
                transport::reply(
                    output,
                    &envelope::failure(Some(id), error),
                    item.as_mut()
                        .context("missing refused configuration lease")?,
                )
                .await?
            }
        }
        Ok(())
    }

    pub(super) async fn cancel(
        &mut self,
        params: Value,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let object = params
            .as_object()
            .context("invalid cancellation parameters")?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "sessionId" | "_meta"))
        {
            bail!("unknown cancellation parameters");
        }
        let requested: CancelNotification = serde_json::from_value(params)?;
        history::validate_id(&requested.session_id.to_string())
            .context("invalid cancellation session identity")?;
        if let Some(session) = self.owned.get_mut(&requested.session_id.to_string()) {
            let active = session.turns.cancel();
            let retired = self.permissions.retire(&session.identity);
            if active {
                tokio::time::timeout(
                    transport::DEADLINE,
                    session.writer.write(&UiCommand::Interrupt),
                )
                .await??;
            }
            for resolution in retired {
                if !active {
                    tokio::time::timeout(
                        transport::DEADLINE,
                        session.writer.write(&resolution.command),
                    )
                    .await??;
                }
                transport::write(output, &resolution.frame).await?;
            }
        }
        Ok(())
    }

    pub(super) fn permission_deadline(&self) -> Option<tokio::time::Instant> {
        self.permissions.deadline()
    }

    pub(super) async fn expire_permissions(
        &mut self,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        for resolution in self.permissions.expire() {
            self.resolve(resolution, output).await?;
        }
        Ok(())
    }

    pub(super) async fn answer(
        &mut self,
        id: &RequestId,
        outcome: &std::result::Result<Value, Value>,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        if let Some(resolution) = self.permissions.answer(id, outcome)? {
            self.resolve(resolution, output).await?;
        }
        Ok(())
    }

    async fn resolve(
        &mut self,
        resolution: permissions::Resolution,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let Some(session) = self.owned.get_mut(&resolution.identity.external) else {
            return Ok(());
        };
        if session.identity != resolution.identity || session.turns.cancelled() {
            return Ok(());
        }
        tokio::time::timeout(
            transport::DEADLINE,
            session.writer.write(&resolution.command),
        )
        .await??;
        transport::write(output, &resolution.frame).await?;
        Ok(())
    }

    pub(super) fn deadline(&self) -> Option<tokio::time::Instant> {
        self.owned
            .values()
            .flat_map(|session| [session.turns.deadline(), session.settings.deadline()])
            .flatten()
            .min()
    }

    pub(super) async fn event(
        &mut self,
        event: events::Event,
        output: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let Some(session) = self.owned.get_mut(&event.identity.external) else {
            return Ok(());
        };
        if session.identity.generation != event.identity.generation
            || session.identity.native != event.identity.native
        {
            return Ok(());
        }
        if let Some(frame) = self.display.boundary(&event.identity.external, &event.body) {
            transport::write(output, &frame).await?;
            return Ok(());
        }
        match &event.body {
            HarnessEvent::ModelChanged { model, .. } => {
                session.settings.model(model.clone())?;
                transport::write(
                    output,
                    &session.settings.notification(&event.identity.external),
                )
                .await?;
                return Ok(());
            }
            HarnessEvent::ModelsRefreshed { choices, .. } => {
                session.settings.catalog(choices.clone())?;
                transport::write(
                    output,
                    &session.settings.notification(&event.identity.external),
                )
                .await?;
                return Ok(());
            }
            HarnessEvent::ModelSelected { request, error } => {
                let response = session.settings.response(*request, error.clone())?;
                transport::reply(output, &response, session.settings.lease()?).await?;
                session.settings.release();
                return Ok(());
            }
            _ => {}
        }
        if let HarnessEvent::PermissionAsked {
            id,
            tool,
            action,
            offers,
            advice,
            ..
        } = event.body
        {
            if session.turns.cancelled() {
                tokio::time::timeout(
                    transport::DEADLINE,
                    session.writer.write(&UiCommand::Permit {
                        id,
                        decision: magi_proto::permit::Decision::Deny,
                    }),
                )
                .await??;
            } else {
                for frame in
                    self.permissions
                        .open(&session.identity, id, tool, action, offers, advice)?
                {
                    transport::write(output, &frame).await?;
                }
            }
            return Ok(());
        }
        let update = session.turns.event(&event.identity.external, event.body)?;
        if update.interrupt {
            for resolution in self.permissions.retire(&session.identity) {
                transport::write(output, &resolution.frame).await?;
            }
            tokio::time::timeout(
                transport::DEADLINE,
                session.writer.write(&UiCommand::Interrupt),
            )
            .await??;
        }
        for frame in update.frames {
            transport::write(output, &frame).await?;
        }
        if let Some(response) = update.complete {
            transport::reply(output, &response, session.turns.lease()?).await?;
        }
        session.turns.release();
        Ok(())
    }

    pub(super) async fn shutdown(&mut self) {
        self.closing.send_replace(true);
        self.opening.clear();
        self.interrupt_owned().await;
        while let Some(opened) = self.pending.join_next().await {
            if let Ok(Opened {
                session: Ok(mut session),
                ..
            }) = opened
            {
                session.reader.finish().await;
                stop(&mut session.child).await;
            }
        }
        for (_, mut session) in std::mem::take(&mut self.owned) {
            session.reader.finish().await;
            stop(&mut session.child).await;
            drop(session);
        }
    }

    async fn interrupt_owned(&mut self) {
        let deadline = tokio::time::Instant::now() + SHUTDOWN;
        for session in self.owned.values_mut() {
            let retired = self.permissions.retire(&session.identity);
            if session.turns.busy() {
                session.turns.cancel();
                let _ =
                    tokio::time::timeout_at(deadline, session.writer.write(&UiCommand::Interrupt))
                        .await;
            } else {
                for resolution in retired {
                    let _ = tokio::time::timeout_at(
                        deadline,
                        session.writer.write(&resolution.command),
                    )
                    .await;
                }
            }
        }
        while self.owned.values().any(|session| session.turns.busy()) {
            let Ok(Some(event)) = tokio::time::timeout_at(deadline, self.incoming.recv()).await
            else {
                break;
            };
            let Some(session) = self.owned.get_mut(&event.identity.external) else {
                continue;
            };
            if session.identity.generation != event.identity.generation
                || session.identity.native != event.identity.native
            {
                continue;
            }
            let _ = session.turns.event(&event.identity.external, event.body);
            session.turns.release();
        }
    }
}

async fn start(
    cwd: PathBuf,
    bootstrap: Bootstrap,
    replay: history::Replay,
    mut closed: watch::Receiver<bool>,
    mut closing: watch::Receiver<bool>,
    events: mpsc::Sender<events::Event>,
    failure: watch::Sender<bool>,
) -> Result<Session> {
    let base = magi_ipc::runtime_dir("magi")?;
    let directory = Directory(magi_ipc::private_dir(
        &base.join(format!("acp-{}", magi_ipc::random_hex(8)?)),
    )?);
    let socket = directory.0.join("host");
    let descriptor = rustix::fs::open(
        &directory.0,
        rustix::fs::OFlags::TMPFILE | rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?;
    let mut input = std::fs::File::from(descriptor);
    input.write_all(&bootstrap.encode()?)?;
    input.rewind()?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args(["--headless", "--tied"])
        .arg(std::process::id().to_string())
        .arg("--socket")
        .arg(&socket)
        .arg("--acp-config")
        .current_dir(cwd)
        .env("CASPER_JAIL", "1")
        .kill_on_drop(true)
        .stdin(Stdio::from(input))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for key in [
        "MAGI_MELCHIOR_SESSION",
        "MAGI_MELCHIOR_PARENT",
        "MAGI_MELCHIOR_TOKEN",
        "MAGI_MELCHIOR_TOKEN_FILE",
        "MAGI_BALTHASAR_INSTANCE",
        "MAGI_MEMORY_INSTANCE",
        "MAGI_API_SOCKET",
        "MAGI_PARENT_HOST_SOCKET",
        "MAGI_HOST_SOCKET",
        "NERV_BALTHASAR_SOCKET",
    ] {
        command.env_remove(key);
    }
    let mut child = command.spawn().context("start native Magi")?;
    let opened = tokio::select! {
        result = tokio::time::timeout(STARTUP, attach(&socket, &mut child, &replay, bootstrap.loading())) => result.context("native startup deadline").and_then(|result| result),
        _ = closed.changed() => Err(anyhow::anyhow!("client disconnected during startup")),
        _ = closing.changed() => Err(anyhow::anyhow!("adapter stopping during startup")),
    };
    match opened {
        Ok(Attached {
            identity,
            reader,
            writer,
            settings,
        }) => Ok(Session {
            child,
            _directory: directory,
            reader: events::Reader::start(reader, identity.clone(), events, failure),
            identity,
            writer,
            turns: turns::Stream::default(),
            settings,
            _mcp: bootstrap,
        }),
        Err(why) => {
            stop(&mut child).await;
            Err(why)
        }
    }
}

async fn attach(
    socket: &std::path::Path,
    child: &mut Child,
    replay: &history::Replay,
    loading: Option<&str>,
) -> Result<Attached> {
    let mut output = child
        .stdout
        .take()
        .context("native readiness pipe unavailable")?;
    let mut ready = [0; bootstrap::READY.len()];
    output
        .read_exact(&mut ready)
        .await
        .context("native readiness handshake")?;
    if ready != bootstrap::READY {
        bail!("invalid native readiness handshake");
    }
    drop(output);
    loop {
        if child.try_wait()?.is_some() {
            bail!("native host exited before attach");
        }
        if let Ok(stream) = magi_ipc::connect(socket).await {
            let key = magi_ipc::read_key(socket)
                .ok_or_else(|| anyhow::anyhow!("native controller key unavailable"))?;
            let (mut read, write) = stream.into_split();
            let mut writer = FrameWriter::new(write);
            writer
                .write(&UiCommand::AttachLatest {
                    draws: false,
                    key: Some(key.clone()),
                })
                .await?;
            let HarnessEvent::SessionSnapshot {
                session,
                cursor,
                model,
                choices,
                ..
            } = FrameReader::new(&mut read).read::<HarnessEvent>().await?
            else {
                bail!("native attach did not return a snapshot");
            };
            let identity = events::Identity {
                external: history::identity(socket, key.clone(), &session, cursor).await?,
                native: session,
                generation: replay.generation,
            };
            if let Some(loading) = loading {
                anyhow::ensure!(
                    identity.external == history::external(loading)?,
                    "native host loaded a different transcript"
                );
            }
            replay.run(socket, &key, &identity, cursor).await?;
            return Ok(Attached {
                identity,
                reader: read,
                writer,
                settings: settings::Settings::new(model, choices)?,
            });
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn stop(child: &mut Child) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    if let Some(pid) = child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::Term);
    }
    if !matches!(
        tokio::time::timeout(SHUTDOWN, child.wait()).await,
        Ok(Ok(_))
    ) {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
