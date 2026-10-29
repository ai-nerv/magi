mod publication;

use super::{
    Collector, Id, MAX_PRODUCERS, Opened, Owner, RegisterError, Sender, Summary,
    retirement::Lifecycle, transport::Channel,
};
use crate::events::Lineage;
use publication::Published;
use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle, Thread},
    time::{Duration, Instant},
};

pub const SHUTDOWN_BUDGET: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(5);

pub(super) struct Lease {
    slots: Arc<AtomicUsize>,
    producers: Arc<AtomicUsize>,
    active: bool,
}

impl Lease {
    fn reserve(readout: &Readout) -> Result<Self, RegisterError> {
        let slots = &readout.slots;
        slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                if n < MAX_PRODUCERS { Some(n + 1) } else { None }
            })
            .map_err(|_| RegisterError::Capacity)?;
        Ok(Self {
            slots: slots.clone(),
            producers: readout.producers.clone(),
            active: false,
        })
    }

    fn activate(&mut self) {
        self.producers.fetch_add(1, Ordering::AcqRel);
        self.active = true;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.active {
            self.producers.fetch_sub(1, Ordering::AcqRel);
        }
        self.slots.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Credit {
    pub identity: Id,
    lease: Lease,
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
    producers: Arc<AtomicUsize>,
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
                .producers
                .load(Ordering::Acquire)
                .saturating_sub(summary.remote_producers);
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
    registrar: Registrar,
    task: Option<JoinHandle<Collector>>,
}

#[derive(Clone)]
pub struct Registrar {
    registrations: Arc<RwLock<SyncSender<Registration>>>,
    stop: Arc<AtomicBool>,
    readout: Readout,
    wake: Thread,
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
            producers: Arc::new(AtomicUsize::new(collector.channels.len())),
        };
        for producer in &mut collector.channels {
            producer._lease = Some(Lease {
                slots: readout.slots.clone(),
                producers: readout.producers.clone(),
                active: true,
            });
        }
        let stopping = stop.clone();
        let task = thread::Builder::new()
            .name("magi-events".into())
            .spawn(move || {
                let _failure = Failure(published.clone());
                run(collector, receiver, stopping, published)
            })?;
        Ok(Self {
            registrar: Registrar {
                registrations: Arc::new(RwLock::new(registrations)),
                stop,
                readout,
                wake: task.thread().clone(),
            },
            task: Some(task),
        })
    }

    pub fn readout(&self) -> Readout {
        self.registrar.readout()
    }

    pub fn registrar(&self) -> Registrar {
        self.registrar.clone()
    }

    /// Queue a fresh producer; keep its lifetime with the native owner.
    pub fn open(&self, owner: Owner) -> Result<Opened, RegisterError> {
        self.registrar.open(owner)
    }

    pub fn open_bound(&self, owner: Owner, lineage: Lineage) -> Result<Opened, RegisterError> {
        self.registrar.open_bound(owner, lineage)
    }

    #[cfg(test)]
    fn register(&self, owner: Owner, identity: Id) -> Result<Sender, RegisterError> {
        self.registrar.register(owner, identity)
    }

    pub fn shutdown(mut self) -> Result<Summary, ShutdownError> {
        self.finish()
    }

    pub(super) fn shutdown_started(mut self, started: Instant) -> Result<Summary, ShutdownError> {
        self.finish_started(started)
    }

    fn finish(&mut self) -> Result<Summary, ShutdownError> {
        self.finish_started(Instant::now())
    }

    fn finish_started(&mut self, started: Instant) -> Result<Summary, ShutdownError> {
        {
            let _admission = self.registrar.registrations.write();
            self.registrar.stop.store(true, Ordering::Release);
        }
        let readout = &self.registrar.readout;
        let Some(task) = self.task.take() else {
            return Ok(readout.snapshot());
        };
        task.thread().unpark();
        match task.join() {
            Ok(collector) => {
                let mut summary = collector.close();
                summary.shutdown_overrun = started.elapsed() > SHUTDOWN_BUDGET;
                readout.published.store(summary);
                if summary.shutdown_overrun {
                    Err(ShutdownError::BudgetExceeded)
                } else {
                    Ok(readout.snapshot())
                }
            }
            Err(_) => {
                let mut summary = readout.published.load();
                summary.closed = true;
                summary.local_unconfirmed = true;
                summary.worker_failed = true;
                summary.shutdown_overrun = started.elapsed() > SHUTDOWN_BUDGET;
                readout.published.store(summary);
                Err(ShutdownError::Panicked)
            }
        }
    }
}

impl Registrar {
    pub fn readout(&self) -> Readout {
        self.readout.clone()
    }

    pub(super) fn available(&self) -> bool {
        !self.stop.load(Ordering::Acquire)
            && self.readout.slots.load(Ordering::Acquire) < MAX_PRODUCERS
    }

    pub(super) fn reserve(&self) -> Result<Credit, RegisterError> {
        self.account_refusal((|| {
            if self.stop.load(Ordering::Acquire) {
                return Err(RegisterError::Closed);
            }
            let lease = Lease::reserve(&self.readout)?;
            let identity = Id::generate().map_err(RegisterError::Entropy)?;
            Ok(Credit { identity, lease })
        })())
    }

    pub(super) fn bind(
        &self,
        credit: Credit,
        owner: Owner,
        lineage: Lineage,
        channel: Channel,
    ) -> Result<super::Lifetime, RegisterError> {
        let (lifecycle, lifetime) = Lifecycle::pair();
        self.submit(credit, owner, lineage, channel, lifecycle)?;
        Ok(lifetime)
    }

    pub(super) fn refuse(&self) {
        let _ = self.account_refusal::<()>(Err(RegisterError::Lineage));
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
        let lease = Lease::reserve(&self.readout)?;
        let (channel, sender) = Channel::pair().map_err(RegisterError::Transport)?;
        self.submit(
            Credit { identity, lease },
            owner,
            lineage,
            channel,
            lifecycle,
        )?;
        Ok(Sender::new(sender, owner, identity).with_lineage(lineage))
    }

    fn submit(
        &self,
        credit: Credit,
        owner: Owner,
        lineage: Lineage,
        channel: Channel,
        lifecycle: Lifecycle,
    ) -> Result<(), RegisterError> {
        let Credit {
            identity,
            mut lease,
        } = credit;
        if !lineage.valid() || !Arc::ptr_eq(&lease.slots, &self.readout.slots) {
            return Err(RegisterError::Lineage);
        }
        lease.activate();
        let registration = Registration {
            owner,
            lineage,
            identity,
            channel,
            lease,
            lifecycle,
        };
        let registrations = self.registrations.try_read().map_err(|_| {
            if self.stop.load(Ordering::Acquire) {
                RegisterError::Closed
            } else {
                RegisterError::Busy
            }
        })?;
        if self.stop.load(Ordering::Acquire) {
            return Err(RegisterError::Closed);
        }
        registrations
            .try_send(registration)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => RegisterError::Busy,
                mpsc::TrySendError::Disconnected(_) => RegisterError::Closed,
            })?;
        self.wake.unpark();
        Ok(())
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
