mod publication;

use super::{
    Collector, Id, MAX_PRODUCERS, Opened, Owner, RegisterError, Sender, Summary,
    retirement::Lifecycle, transport::Channel,
};
use crate::events::Lineage;
use publication::Published;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const SHUTDOWN_BUDGET: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(5);

pub(super) struct Lease(Arc<AtomicUsize>);

impl Lease {
    fn reserve(slots: &Arc<AtomicUsize>) -> Result<Self, RegisterError> {
        slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                if n < MAX_PRODUCERS { Some(n + 1) } else { None }
            })
            .map_err(|_| RegisterError::Capacity)?;
        Ok(Self(slots.clone()))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Registration {
    pub owner: Owner,
    pub lineage: Lineage,
    pub identity: Id,
    pub channel: Channel,
    pub lease: Lease,
    pub lifecycle: Lifecycle,
}

#[derive(Clone)]
pub struct Readout {
    published: Arc<Published>,
    refused: Arc<AtomicU64>,
    slots: Arc<AtomicUsize>,
}

impl Readout {
    pub fn snapshot(&self) -> Summary {
        let mut summary = self.published.load();
        summary.counts.refused_producers = summary
            .counts
            .refused_producers
            .saturating_add(self.refused.load(Ordering::Relaxed));
        if !summary.closed {
            let pending = self
                .slots
                .load(Ordering::Acquire)
                .saturating_sub(1 + summary.remote_producers);
            summary.remote_producers = summary.remote_producers.saturating_add(pending);
            summary.final_confirmed = summary.final_confirmed.min(summary.remote_producers);
            summary.unknown_loss_producers = summary.remote_producers - summary.final_confirmed;
            summary.reported_loss_producers = summary
                .reported_loss_producers
                .min(summary.remote_producers);
        }
        summary
    }
}

pub struct Worker {
    registrations: SyncSender<Registration>,
    stop: Arc<AtomicBool>,
    readout: Readout,
    task: Option<JoinHandle<Collector>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownError {
    Panicked,
    BudgetExceeded,
}

impl Worker {
    pub fn start(collector: Collector) -> std::io::Result<Self> {
        Self::spawn(collector, run)
    }

    fn spawn(
        mut collector: Collector,
        run: impl FnOnce(
            Collector,
            mpsc::Receiver<Registration>,
            Arc<AtomicBool>,
            Arc<Published>,
        ) -> Collector
        + Send
        + 'static,
    ) -> std::io::Result<Self> {
        let (registrations, receiver) = mpsc::sync_channel(MAX_PRODUCERS);
        let stop = Arc::new(AtomicBool::new(false));
        let published = Arc::new(Published::default());
        published.store(collector.summary());
        let readout = Readout {
            published: published.clone(),
            refused: Arc::new(AtomicU64::new(0)),
            slots: Arc::new(AtomicUsize::new(1 + collector.channels.len())),
        };
        for producer in &mut collector.channels {
            producer._lease = Some(Lease(readout.slots.clone()));
        }
        let stopping = stop.clone();
        let task = thread::Builder::new()
            .name("magi-events".into())
            .spawn(move || {
                let _failure = Failure(published.clone());
                run(collector, receiver, stopping, published)
            })?;
        Ok(Self {
            registrations,
            stop,
            readout,
            task: Some(task),
        })
    }

    pub fn readout(&self) -> Readout {
        self.readout.clone()
    }

    /// Queue a fresh producer; keep its lifetime with the native owner.
    pub fn open(&self, owner: Owner) -> Result<Opened, RegisterError> {
        self.open_bound(owner, Lineage::default())
    }

    pub fn open_bound(&self, owner: Owner, lineage: Lineage) -> Result<Opened, RegisterError> {
        if !lineage.valid() {
            return self.account_refusal(Err(RegisterError::Lineage));
        }
        let result = Id::generate()
            .map_err(RegisterError::Entropy)
            .and_then(|identity| {
                let (lifecycle, lifetime) = Lifecycle::pair();
                self.enqueue(owner, identity, lifecycle, lineage)
                    .map(|sender| Opened { sender, lifetime })
            });
        self.account_refusal(result)
    }

    #[cfg(test)]
    fn register(&self, owner: Owner, identity: Id) -> Result<Sender, RegisterError> {
        self.account_refusal(self.enqueue(owner, identity, Lifecycle::new(), Lineage::default()))
    }

    fn account_refusal<T>(&self, result: Result<T, RegisterError>) -> Result<T, RegisterError> {
        if result.is_err() {
            let _ = self
                .readout
                .refused
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    Some(n.saturating_add(1))
                });
        }
        result
    }

    fn enqueue(
        &self,
        owner: Owner,
        identity: Id,
        lifecycle: Lifecycle,
        lineage: Lineage,
    ) -> Result<Sender, RegisterError> {
        if self.stop.load(Ordering::Acquire) {
            return Err(RegisterError::Closed);
        }
        let lease = Lease::reserve(&self.readout.slots)?;
        let (channel, sender) = Channel::pair().map_err(RegisterError::Transport)?;
        let registration = Registration {
            owner,
            lineage,
            identity,
            channel,
            lease,
            lifecycle,
        };
        self.registrations
            .try_send(registration)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => RegisterError::Busy,
                mpsc::TrySendError::Disconnected(_) => RegisterError::Closed,
            })?;
        if let Some(task) = &self.task {
            task.thread().unpark();
        }
        Ok(Sender::new(sender, owner, identity).with_lineage(lineage))
    }

    pub fn shutdown(mut self) -> Result<Summary, ShutdownError> {
        self.finish()
    }

    fn finish(&mut self) -> Result<Summary, ShutdownError> {
        self.finish_started(Instant::now())
    }

    fn finish_started(&mut self, started: Instant) -> Result<Summary, ShutdownError> {
        self.stop.store(true, Ordering::Release);
        let Some(task) = self.task.take() else {
            return Ok(self.readout.snapshot());
        };
        task.thread().unpark();
        match task.join() {
            Ok(collector) => {
                let mut summary = collector.close();
                summary.shutdown_overrun = started.elapsed() > SHUTDOWN_BUDGET;
                self.readout.published.store(summary);
                if summary.shutdown_overrun {
                    Err(ShutdownError::BudgetExceeded)
                } else {
                    Ok(self.readout.snapshot())
                }
            }
            Err(_) => {
                let mut summary = self.readout.published.load();
                summary.closed = true;
                summary.local_unconfirmed = true;
                summary.worker_failed = true;
                summary.shutdown_overrun = started.elapsed() > SHUTDOWN_BUDGET;
                self.readout.published.store(summary);
                Err(ShutdownError::Panicked)
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.task.is_some() {
            let _ = self.finish();
        }
    }
}

struct Failure(Arc<Published>);

impl Drop for Failure {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.fail();
        }
    }
}

fn registrations(collector: &mut Collector, receiver: &mpsc::Receiver<Registration>) {
    for _ in 0..MAX_PRODUCERS {
        let Ok(registration) = receiver.try_recv() else {
            break;
        };
        collector.admit(registration);
    }
}

fn run(
    mut collector: Collector,
    receiver: mpsc::Receiver<Registration>,
    stop: Arc<AtomicBool>,
    published: Arc<Published>,
) -> Collector {
    while !stop.load(Ordering::Acquire) {
        registrations(&mut collector, &receiver);
        collector.poll_while(Instant::now(), || !stop.load(Ordering::Acquire));
        collector.retire(Instant::now());
        published.store(collector.summary());
        if !stop.load(Ordering::Acquire) {
            thread::park_timeout(POLL_INTERVAL);
        }
    }
    registrations(&mut collector, &receiver);
    collector
}

#[cfg(test)]
mod tests;
