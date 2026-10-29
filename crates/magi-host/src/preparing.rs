use crate::{cancel::Cancel, catalog::Catalog, refreshing::Models, session::Session};
use magi_proto::{Entry, MessageId, UiCommand};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore, mpsc, oneshot};

const COUNT: usize = 256;
const BYTES: usize = 64 * 1024 * 1024;
mod listing;
use crate::{events::TurnOutcome, session::observing::Trace};
use listing::{Response, discover, warnings};

pub(super) type Completed = (Ticket, Result<Response, String>);

struct Budget {
    _count: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}

struct Epoch {
    cancel: std::sync::Mutex<Cancel>,
    boundary: Mutex<()>,
}

impl Epoch {
    fn current(&self) -> Cancel {
        self.cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn invalidate(&self) {
        let mut cancel = self.cancel.lock().unwrap_or_else(|p| p.into_inner());
        cancel.request();
        *cancel = Cancel::default();
    }
}

struct Job {
    trace: Trace,
    command: UiCommand,
    tracking: Option<u64>,
    cancel: Cancel,
    epoch: Cancel,
    answer: oneshot::Sender<Result<Response, String>>,
    _budget: Arc<Budget>,
    deadline: tokio::time::Instant,
}

pub(super) struct Ticket {
    answer: oneshot::Receiver<Result<Response, String>>,
    cancel: Cancel,
    _budget: Arc<Budget>,
    pub(super) listing: bool,
    pub(super) tracking: Option<u64>,
}

impl Ticket {
    pub(super) async fn receive(&mut self) -> Result<Response, String> {
        (&mut self.answer)
            .await
            .map_err(|_| "prompt preparation stopped".to_owned())?
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel.request();
    }
}

pub(super) enum Queued {
    Immediate(UiCommand, Option<u64>, Trace),
    Waiting(Ticket),
}

pub(super) struct Preparing {
    sender: mpsc::Sender<Job>,
    epoch: Arc<Epoch>,
    count: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    running: tokio::task::JoinHandle<()>,
}

impl Preparing {
    pub(super) fn new(
        session: Arc<Mutex<Session>>,
        worker: Arc<RwLock<Option<Arc<crate::worker::Worker>>>>,
        models: Arc<Models>,
        scribe: crate::scribe::Held,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Job>(COUNT);
        let epoch = Arc::new(Epoch {
            cancel: Default::default(),
            boundary: Mutex::new(()),
        });
        let state = Arc::clone(&epoch);
        let running = tokio::spawn(async move {
            while let Some(job) = receiver.recv().await {
                let mut trace = job.trace;
                let result = async {
                    let catalog = {
                        let _boundary = state.boundary.lock().await;
                        if job.cancel.is_requested() || job.epoch.is_requested() {
                            return Err("prompt preparation was cancelled".into());
                        }
                        if workflow(&job.command)? && scribe.lock().await.is_none() {
                            return Err(
                                "workflow prompts require a configured history store".into()
                            );
                        }
                        models.catalog.read().await.clone()
                    };
                    if matches!(job.command, UiCommand::RefreshWorkflows) {
                        let (_, catalog) = discover(&catalog, &job.cancel).await?;
                        let _boundary = state.boundary.lock().await;
                        if job.cancel.is_requested() || job.epoch.is_requested() {
                            return Err("workflow catalog preparation was cancelled".into());
                        }
                        return listing::response(&catalog);
                    }
                    let (request, warnings) = resolve(job.command, &catalog, &job.cancel).await?;
                    let _boundary = state.boundary.lock().await;
                    crate::turning::submit(
                        &session,
                        request.tracked(job.tracking),
                        &worker,
                        &catalog,
                        &scribe,
                        &[job.cancel.clone(), job.epoch.clone()],
                        &mut trace,
                    )
                    .await?;
                    Ok(Response::Admitted(warnings))
                };
                let (answer, terminal) = tokio::select! {
                    biased;
                    _ = job.cancel.requested() => (Err("prompt preparation was cancelled".into()), Some(TurnOutcome::Cancelled)),
                    _ = job.epoch.requested() => (Err("session or configuration changed during prompt preparation".into()), Some(TurnOutcome::Cancelled)),
                    _ = tokio::time::sleep_until(job.deadline) => (Err("prompt preparation deadline elapsed".into()), Some(TurnOutcome::Expired)),
                    result = result => {
                        let terminal = result.is_err().then_some(TurnOutcome::Refused);
                        (result, terminal)
                    },
                };
                if let Some(outcome) = terminal {
                    trace.finish(outcome);
                }
                let _ = job.answer.send(answer);
            }
        });
        Self {
            sender,
            epoch,
            count: Arc::new(Semaphore::new(COUNT)),
            bytes: Arc::new(Semaphore::new(BYTES)),
            running,
        }
    }

    pub(super) fn queue(&self, command: UiCommand) -> Result<Queued, String> {
        let mut trace = Trace::command(&command);
        let (command, tracking) = match command {
            UiCommand::SubmitTracked {
                request,
                text,
                aside,
                images,
            } => (
                UiCommand::SubmitPrompt {
                    text,
                    aside,
                    images,
                },
                Some(request),
            ),
            command => (command, None),
        };
        let selected = workflow(&command)?;
        let listing = matches!(command, UiCommand::RefreshWorkflows);
        if !selected && !listing && self.count.available_permits() == COUNT {
            return Ok(Queued::Immediate(command, tracking, trace));
        }
        if selected
            && let UiCommand::SubmitPrompt {
                text,
                aside,
                images,
            } = &command
        {
            magi_model::validate_images(images)?;
            if text.len()
                + aside.len()
                + images
                    .iter()
                    .map(|image| image.data.len() + image.media_type.len())
                    .sum::<usize>()
                > crate::session::admission::ENTRY_MOST
            {
                return Err("workflow prompt exceeds the admission byte limit".into());
            }
        }
        let size = serde_json::to_vec(&command)
            .map_err(|why| why.to_string())?
            .len();
        let size = size.max(512) + if listing { listing::MOST } else { 0 };
        let size = u32::try_from(size).map_err(|_| "prompt preparation is too large")?;
        let budget = Arc::new(Budget {
            _count: Arc::clone(&self.count)
                .try_acquire_owned()
                .map_err(|_| "prompt preparation queue is full")?,
            _bytes: Arc::clone(&self.bytes)
                .try_acquire_many_owned(size)
                .map_err(|_| "prompt preparation byte budget is full")?,
        });
        let cancel = Cancel::default();
        let (answer, receive) = oneshot::channel();
        trace.preparing();
        self.sender
            .try_send(Job {
                trace,
                command,
                tracking,
                cancel: cancel.clone(),
                epoch: self.epoch.current(),
                answer,
                _budget: Arc::clone(&budget),
                deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(45),
            })
            .map_err(|error| {
                error.into_inner().trace.finish(TurnOutcome::Refused);
                "prompt preparation queue is unavailable"
            })?;
        Ok(Queued::Waiting(Ticket {
            answer: receive,
            cancel,
            _budget: budget,
            listing,
            tracking,
        }))
    }

    pub(super) fn invalidate(&self) {
        self.epoch.invalidate();
    }

    pub(super) async fn changing(&self) -> tokio::sync::MutexGuard<'_, ()> {
        let boundary = self.epoch.boundary.lock().await;
        self.invalidate();
        boundary
    }
}

impl Drop for Preparing {
    fn drop(&mut self) {
        self.invalidate();
        self.running.abort();
    }
}

fn workflow(command: &UiCommand) -> Result<bool, String> {
    match command {
        UiCommand::SubmitPrompt { text, .. } => crate::skills::invocation(text)
            .map(|invocation| invocation.is_some())
            .map_err(|why| why.to_string()),
        _ => Ok(false),
    }
}

pub(super) fn request(command: UiCommand) -> Result<crate::session::admission::Request, String> {
    use crate::session::admission::Request;
    Ok(match command {
        UiCommand::SubmitPrompt {
            text,
            aside,
            images,
        } => Request::Opening(Entry::User {
            id: MessageId::new("queued"),
            text,
            aside,
            images,
        }),
        UiCommand::Arrived {
            who,
            kin,
            sort,
            text,
        } => Request::Opening(Entry::From {
            who,
            kin,
            sort,
            text,
        }),
        UiCommand::TakeGrants { grants } => Request::Grants(grants),
        UiCommand::DeclareNeeds => Request::Declare,
        _ => return Err("not a prompt admission command".into()),
    })
}

async fn resolve(
    command: UiCommand,
    catalog: &Catalog,
    cancel: &Cancel,
) -> Result<(crate::session::admission::Request, Vec<String>), String> {
    let UiCommand::SubmitPrompt {
        text,
        aside,
        images,
    } = command
    else {
        return Ok((request(command)?, Vec::new()));
    };
    let Some(invocation) = crate::skills::invocation(&text).map_err(|why| why.to_string())? else {
        return Ok((
            request(UiCommand::SubmitPrompt {
                text,
                aside,
                images,
            })?,
            Vec::new(),
        ));
    };
    let (client, selected) = discover(catalog, cancel).await?;
    let loaded = selected
        .load(&client, invocation.name, cancel)
        .await
        .map_err(|why| why.to_string())?;
    let workflow = loaded.snapshot(invocation.arguments.into())?;
    let warnings = warnings(&selected);
    Ok((
        crate::session::admission::Request::Opening(Entry::Workflow {
            id: MessageId::new("queued"),
            text,
            aside,
            images,
            workflow: Box::new(workflow),
        }),
        warnings,
    ))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod observing;
