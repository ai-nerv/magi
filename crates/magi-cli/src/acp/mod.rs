//! ACP stdio admission and protocol state.

pub(crate) mod bootstrap;
mod descriptors;
mod display;
mod envelope;
mod events;
mod history;
mod initialize;
mod intake;
mod json;
mod permissions;
mod prompt;
mod sessions;
mod settings;
mod transport;
mod turns;

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncWrite};

pub async fn stdio() -> Result<()> {
    let reader = descriptors::Pollable::duplicate(std::io::stdin())?;
    let writer = descriptors::Pollable::duplicate(std::io::stdout())?;
    eprintln!(
        "magi acp: experimental adapter; general questions and interactive surfaces are unsupported"
    );
    serve(reader, writer).await
}

async fn serve(
    reader: impl AsyncRead + Unpin + Send + 'static,
    mut writer: impl AsyncWrite + Unpin,
) -> Result<()> {
    let mut intake = intake::Intake::new(reader);
    let mut sessions = sessions::Sessions::default();
    let result = dispatch(&mut intake, &mut writer, &mut sessions).await;
    sessions.shutdown().await;
    intake.finish().await;
    result
}

async fn dispatch(
    intake: &mut intake::Intake,
    writer: &mut (impl AsyncWrite + Unpin),
    sessions: &mut sessions::Sessions,
) -> Result<()> {
    let mut state = initialize::State::default();
    let admission_deadline = tokio::time::Instant::now() + transport::DEADLINE;
    loop {
        let deadline = if state.ready() {
            sessions.deadline()
        } else {
            Some(admission_deadline)
        };
        let permission_deadline = sessions.permission_deadline();
        let item = tokio::select! {
            item = intake.incoming.recv() => item,
            event = sessions.incoming.recv() => {
                if let Some(event) = event { sessions.event(event, writer).await?; }
                continue;
            }
            replay = sessions.replayed.recv() => {
                if let Some(replay) = replay { sessions.replay(replay, writer).await?; }
                continue;
            }
            opened = sessions.pending.join_next(), if !sessions.pending.is_empty() => {
                if let Some(opened) = opened { sessions.opened(opened?, writer).await?; }
                continue;
            }
            _ = sessions.failed.changed() => bail!("native event stream failed"),
            _ = async {
                match permission_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                sessions.expire_permissions(writer).await?;
                continue;
            }
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => bail!("ACP admission or cancellation deadline elapsed"),
        };
        let Some(mut item) = item else {
            return intake.ended().await;
        };
        if state.ready()
            && let Ok(envelope::Incoming::Request { id, method, params }) = &item.message
        {
            match method.as_str() {
                "session/set_config_option" => {
                    sessions
                        .configure(id.clone(), params.clone(), item, writer)
                        .await?;
                    continue;
                }
                "session/prompt" => {
                    sessions
                        .prompt(id.clone(), params.clone(), item, writer)
                        .await?;
                    continue;
                }
                "session/new" | "session/load" => {
                    sessions
                        .create(
                            id.clone(),
                            if method == "session/load" {
                                sessions::Opening::Load(params.clone())
                            } else {
                                sessions::Opening::New(params.clone())
                            },
                            item,
                            intake.closed.clone(),
                            writer,
                        )
                        .await?;
                    continue;
                }
                _ => {}
            }
        }
        let response = match &item.message {
            Err(error) => Some(error.clone()),
            Ok(envelope::Incoming::Response { id, outcome }) => {
                sessions.answer(id, outcome, writer).await?;
                None
            }
            Ok(envelope::Incoming::Notification { method, params }) => {
                if state.ready() && method == "session/cancel" {
                    sessions.cancel(params.clone(), writer).await?;
                }
                None
            }
            Ok(envelope::Incoming::Request { id, method, params }) => {
                let response = state.request(id.clone(), method, params.clone());
                sessions.display = state.display();
                Some(response)
            }
        };
        if let Some(response) = response {
            transport::reply(writer, &response, &mut item).await?;
        }
    }
}

#[cfg(test)]
mod tests;
