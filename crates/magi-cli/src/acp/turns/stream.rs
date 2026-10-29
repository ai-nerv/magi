use super::*;
use anyhow::Context;

#[derive(Default)]
pub(in crate::acp) struct Stream {
    next: u64,
    last_owner: u64,
    pending: Option<(u64, Turn)>,
    active: Option<(u64, Turn)>,
    settled: Option<Turn>,
}

impl Stream {
    pub(in crate::acp) fn busy(&self) -> bool {
        self.pending.is_some() || self.active.is_some()
    }

    pub(in crate::acp) fn submit(&mut self, id: RequestId, lease: Item) -> Result<u64> {
        if self.busy() || self.settled.is_some() {
            bail!("native stream already owns a prompt");
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("prompt identity exhausted"))?;
        self.pending = Some((self.next, Turn::new(id, lease)));
        Ok(self.next)
    }

    pub(in crate::acp) fn cancel(&mut self) -> bool {
        let pending = self.pending.as_mut().is_some_and(|(_, turn)| turn.cancel());
        let active = self.active.as_mut().is_some_and(|(_, turn)| turn.cancel());
        pending || active
    }

    pub(in crate::acp) fn cancelled(&self) -> bool {
        self.pending
            .iter()
            .chain(self.active.iter())
            .any(|(_, turn)| turn.cancelled())
    }

    pub(in crate::acp) fn deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .chain(self.active.iter())
            .filter_map(|(_, turn)| turn.deadline)
            .min()
    }

    pub(in crate::acp) fn release(&mut self) {
        self.settled = None;
    }

    pub(in crate::acp) fn lease(&mut self) -> Result<&mut Item> {
        self.settled
            .as_mut()
            .and_then(|turn| turn._lease.as_mut())
            .context("missing settled prompt lease")
    }

    pub(in crate::acp) fn event(&mut self, session: &str, event: HarnessEvent) -> Result<Update> {
        let empty = || Update {
            frames: Vec::new(),
            complete: None,
            interrupt: false,
        };
        match event {
            HarnessEvent::TurnStarted { owner, request } => {
                if self.active.is_some() || owner <= self.last_owner {
                    bail!("overlapping native turn boundaries");
                }
                let turn = if let Some(request) = request {
                    if self.pending.as_ref().map(|(key, _)| *key) != Some(request) {
                        bail!("unknown native prompt admission");
                    }
                    self.pending.take().context("pending prompt missing")?.1
                } else {
                    Turn::background()
                };
                self.last_owner = owner;
                self.active = Some((owner, turn));
                Ok(empty())
            }
            HarnessEvent::TurnFinished { owner } => {
                if self.active.as_ref().map(|(key, _)| *key) != Some(owner) {
                    bail!("uncorrelated native turn completion");
                }
                let (_, turn) = self.active.take().context("active turn missing")?;
                let update = turn.finish(session);
                self.settled = Some(turn);
                Ok(update)
            }
            HarnessEvent::PromptRefused { request, .. } => {
                if self.pending.as_ref().map(|(key, _)| *key) != Some(request) {
                    bail!("uncorrelated native prompt refusal");
                }
                let (_, mut turn) = self.pending.take().context("pending prompt missing")?;
                turn.failed = true;
                let update = turn.finish(session);
                self.settled = Some(turn);
                Ok(update)
            }
            event => match self.active.as_mut() {
                Some((_, turn)) => turn.event(session, event),
                None => match event {
                    HarnessEvent::SessionSnapshot { .. }
                    | HarnessEvent::Asked { .. }
                    | HarnessEvent::Surfaced { .. }
                    | HarnessEvent::AssistantStarted { .. }
                    | HarnessEvent::AssistantDelta { .. }
                    | HarnessEvent::AssistantEnded { .. }
                    | HarnessEvent::UserMessage { .. }
                    | HarnessEvent::WorkflowMessage { .. }
                    | HarnessEvent::MessageArrived { .. }
                    | HarnessEvent::ToolCallStarted { .. }
                    | HarnessEvent::ToolCallEnded { .. }
                    | HarnessEvent::Refused { .. }
                    | HarnessEvent::Error { .. } => bail!("native event outside a turn boundary"),
                    _ => Ok(empty()),
                },
            },
        }
    }
}
