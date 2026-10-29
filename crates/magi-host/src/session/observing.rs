use super::Session;
use crate::events::{self, Correlation, Fact, Id, TurnOutcome};
use magi_proto::{Cursor, Entry, StopReason, UiCommand};
use std::time::Instant;

#[derive(Default)]
enum Stage {
    #[default]
    Unadmitted,
    Preparing,
    Queued,
    Started,
    Finished,
}

#[derive(Default)]
pub(crate) struct Trace {
    id: Option<Id>,
    began: Option<Instant>,
    stage: Stage,
}

impl Trace {
    fn new(prompt: bool) -> Self {
        let id = if prompt { events::identity() } else { None };
        Self {
            id,
            began: id.map(|_| Instant::now()),
            stage: Stage::Unadmitted,
        }
    }

    pub(crate) fn command(command: &UiCommand) -> Self {
        Self::new(matches!(
            command,
            UiCommand::SubmitPrompt { .. } | UiCommand::SubmitTracked { .. }
        ))
    }

    #[cfg(test)]
    pub(super) fn request(request: &super::admission::Request) -> Self {
        Self::new(matches!(
            request.entry(),
            Some(Entry::User { .. } | Entry::Workflow { .. })
        ))
    }

    pub(crate) fn preparing(&mut self) {
        self.stage = Stage::Preparing;
        self.emit(TurnOutcome::Preparing, false);
    }

    pub(super) fn admission(&mut self) {
        self.stage = Stage::Unadmitted;
    }

    fn emit(&self, outcome: TurnOutcome, terminal: bool) {
        if let Some(id) = self.id {
            events::emit(|| {
                (
                    Correlation {
                        turn_id: Some(id),
                        ..Correlation::default()
                    },
                    Fact::LogicalTurn {
                        outcome,
                        duration_ms: if terminal {
                            self.began.and_then(events::elapsed)
                        } else {
                            None
                        },
                    },
                )
            });
        }
    }

    pub(super) fn admitted(&mut self, queued: bool) {
        self.stage = Stage::Queued;
        self.emit(TurnOutcome::Admitted, false);
        if queued {
            self.emit(TurnOutcome::Queued, false);
        }
    }

    pub(super) fn start(&mut self) {
        self.stage = Stage::Started;
        self.emit(TurnOutcome::Started, false);
    }

    pub(crate) fn finish(&mut self, outcome: TurnOutcome) {
        if !matches!(self.stage, Stage::Finished) {
            self.stage = Stage::Finished;
            self.emit(outcome, true);
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.id.is_some()
    }

    pub(super) fn identity(&self) -> Option<Id> {
        self.id
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        let outcome = match self.stage {
            Stage::Unadmitted => TurnOutcome::Refused,
            Stage::Preparing => TurnOutcome::Unknown,
            Stage::Queued => TurnOutcome::Cancelled,
            Stage::Started => TurnOutcome::Unknown,
            Stage::Finished => return,
        };
        self.finish(outcome);
    }
}

impl Session {
    pub(crate) fn observe_finished(&mut self, owner: u64, began: Cursor, failed: bool) {
        if !self.admission.observing(owner) {
            return;
        }
        let last = self
            .entries()
            .iter()
            .enumerate()
            .rev()
            .map_while(|(at, entry)| {
                self.cursor_at(at)
                    .filter(|&cursor| cursor > began)
                    .map(|_| entry)
            })
            .find(|entry| matches!(entry, Entry::Assistant { .. }));
        let outcome = terminal(last, failed, self.cancel.is_requested());
        self.admission.observed(outcome);
    }
}

fn terminal(last: Option<&Entry>, failed: bool, cancelled: bool) -> TurnOutcome {
    if failed {
        return TurnOutcome::Failed;
    }
    match last {
        Some(
            Entry::Assistant { error: Some(_), .. }
            | Entry::Assistant {
                stop_reason: Some(StopReason::Error),
                ..
            },
        ) => TurnOutcome::Failed,
        Some(Entry::Assistant {
            stop_reason: Some(StopReason::Aborted),
            ..
        }) => TurnOutcome::Cancelled,
        Some(Entry::Assistant {
            stop_reason: Some(StopReason::EndTurn | StopReason::Length),
            ..
        }) => TurnOutcome::Completed,
        _ if cancelled => TurnOutcome::Cancelled,
        _ => TurnOutcome::Unknown,
    }
}

#[cfg(test)]
mod tests;
