use super::{
    Owner, REPORT_INTERVAL, Report, SHUTDOWN_BUDGET, Sender, ShutdownError, TransportLoss,
    watch::Watch,
};
use crate::events::{EnableError, Loss, Receiver};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("event sender already used")]
    UsedSender,
    #[error("event sender belongs to a different owner")]
    Owner,
    #[error("local event activation failed")]
    Enable(EnableError),
    #[error("event forwarding worker could not start")]
    Thread(std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forwarded {
    pub queue_loss: Loss,
    pub transport_loss: TransportLoss,
    pub final_report: Report,
    pub local_unconfirmed: bool,
    pub worker_failed: bool,
}

impl Forwarded {
    pub fn incomplete(&self) -> bool {
        self.local_unconfirmed
            || self.worker_failed
            || self.queue_loss != Loss::default()
            || self.transport_loss != TransportLoss::default()
            || self.final_report != Report::Sent(super::Sent::Queued)
    }
}

pub struct Forwarder {
    stop: Arc<AtomicBool>,
    task: Option<JoinHandle<(Receiver, Sender)>>,
    watch: Watch,
}

impl Forwarder {
    /// Activate the sole local queue with the native parent's producer binding.
    pub fn activate(sender: Sender) -> Result<Self, ForwardError> {
        Self::activate_watched(sender, Watch::default())
    }

    pub(super) fn activate_watched(sender: Sender, watch: Watch) -> Result<Self, ForwardError> {
        let lineage = sender.lineage();
        let (inner, owner, producer) = sender.into_fresh().ok_or(ForwardError::UsedSender)?;
        if owner != Owner::Magi {
            return Err(ForwardError::Owner);
        }
        let receiver =
            crate::events::enable_bound(producer, lineage).map_err(ForwardError::Enable)?;
        Self::spawn_watched(
            receiver,
            Sender::new(inner, owner, producer).with_lineage(lineage),
            watch,
            run,
        )
        .map_err(ForwardError::Thread)
    }

    #[cfg(test)]
    fn spawn(
        receiver: Receiver,
        sender: Sender,
        run: impl FnOnce(Receiver, Sender, Arc<AtomicBool>) -> (Receiver, Sender) + Send + 'static,
    ) -> std::io::Result<Self> {
        Self::spawn_watched(receiver, sender, Watch::default(), run)
    }

    fn spawn_watched(
        receiver: Receiver,
        sender: Sender,
        watch: Watch,
        run: impl FnOnce(Receiver, Sender, Arc<AtomicBool>) -> (Receiver, Sender) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let guard = watch.pending(stopping.clone());
        let task = thread::Builder::new()
            .name("magi-event-send".into())
            .spawn(move || {
                let _guard = guard.enter();
                run(receiver, sender, stopping)
            })?;
        Ok(Self {
            stop,
            task: Some(task),
            watch,
        })
    }

    pub fn shutdown(mut self) -> Result<Forwarded, ShutdownError> {
        self.finish(Instant::now())
    }

    pub(super) fn shutdown_started(mut self, started: Instant) -> Result<Forwarded, ShutdownError> {
        self.finish(started)
    }

    fn finish(&mut self, started: Instant) -> Result<Forwarded, ShutdownError> {
        self.stop.store(true, Ordering::Release);
        let Some(task) = self.task.take() else {
            return Err(ShutdownError::Panicked);
        };
        task.thread().unpark();
        let (receiver, mut sender) = task.join().map_err(|_| ShutdownError::Panicked)?;
        let counters = receiver.loss.clone();
        drop(receiver);
        let local_unconfirmed = counters.active.load(Ordering::SeqCst) != 0;
        let queue_loss = counters.snapshot();
        let worker_failed = self.watch.failed();
        let finished = !worker_failed && !local_unconfirmed && started.elapsed() <= SHUTDOWN_BUDGET;
        let final_report = sender.report(queue_loss, finished);
        let outcome = Forwarded {
            queue_loss,
            transport_loss: sender.loss(),
            final_report,
            local_unconfirmed,
            worker_failed,
        };
        if started.elapsed() > SHUTDOWN_BUDGET {
            Err(ShutdownError::BudgetExceeded)
        } else {
            Ok(outcome)
        }
    }
}

impl Drop for Forwarder {
    fn drop(&mut self) {
        if self.task.is_some() {
            let _ = self.finish(Instant::now());
        }
    }
}

fn run(receiver: Receiver, mut sender: Sender, stop: Arc<AtomicBool>) -> (Receiver, Sender) {
    let mut reported = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if let Ok(bytes) = receiver.try_recv() {
            sender.send(&bytes);
        }
        if reported.elapsed() >= REPORT_INTERVAL {
            sender.report(receiver.loss.snapshot(), false);
            reported = Instant::now();
        }
        if !stop.load(Ordering::Acquire) {
            thread::park_timeout(POLL_INTERVAL);
        }
    }
    (receiver, sender)
}

#[cfg(test)]
mod tests;
