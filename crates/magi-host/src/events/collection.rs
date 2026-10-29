pub mod bootstrap;
pub mod branch;
pub mod enrollment;
mod forwarding;
mod retirement;
pub mod runtime;
mod status;
mod transport;
mod watch;
mod wire;
mod worker;

pub use forwarding::{ForwardError, Forwarded, Forwarder};
pub use retirement::{Lifetime, Opened, RETIREMENT_BUDGET, Retired};
pub use status::{Delivery, ProducerStatus, REPORT_INTERVAL, Report, Sender};
pub use transport::{Loss as TransportLoss, Sent};
pub use wire::{Invalid, Ledger, Owner, Usage};
pub use worker::{Readout, Registrar, SHUTDOWN_BUDGET, ShutdownError, Worker};

use super::{Id, Lineage, MAX_EVENT_BYTES, Receiver};
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroU64,
    time::{Duration, Instant},
};
use transport::{Channel, Received};

pub const MAX_PRODUCERS: usize = 32;
pub const MAX_SEEN: usize = 4096;
pub const MAX_ASKS: usize = 1024;
pub const RETENTION: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub accepted: u64,
    pub duplicate: u64,
    pub invalid: u64,
    pub unavailable: u64,
    pub refused_producers: u64,
    pub dedup_evicted: u64,
    pub usage_evicted: u64,
    pub usage_unattributed: u64,
    pub usage_stale: u64,
    pub usage_conflicts: u64,
    pub statuses: u64,
    pub status_stale: u64,
}

#[derive(Debug)]
pub enum RegisterError {
    Lineage,
    Capacity,
    Duplicate,
    Busy,
    Closed,
    Entropy(std::io::Error),
    Transport(std::io::Error),
}

struct Producer {
    owner: Owner,
    lineage: Lineage,
    identity: Id,
    channel: Channel,
    delivery: Delivery,
    lifecycle: retirement::Lifecycle,
    _lease: Option<worker::Lease>,
}

struct Snapshot {
    producer: Id,
    ask: Id,
    usage: Usage,
    at: Instant,
}

type EventKey = (Id, NonZeroU64);

pub struct Collector {
    local: Receiver,
    channels: Vec<Producer>,
    seen: HashMap<EventKey, Instant>,
    order: VecDeque<(EventKey, Instant)>,
    usage: VecDeque<Snapshot>,
    counts: Counts,
    retired: Retired,
}

impl Collector {
    pub fn new(local: Receiver) -> Self {
        Self {
            local,
            channels: Vec::new(),
            seen: HashMap::new(),
            order: VecDeque::new(),
            usage: VecDeque::new(),
            counts: Counts::default(),
            retired: Retired::default(),
        }
    }

    pub fn open(&mut self, owner: Owner) -> Result<Opened, RegisterError> {
        self.open_bound(owner, Lineage::default())
    }

    pub fn open_bound(&mut self, owner: Owner, lineage: Lineage) -> Result<Opened, RegisterError> {
        if !lineage.valid() {
            increment(&mut self.counts.refused_producers);
            return Err(RegisterError::Lineage);
        }
        let identity = Id::generate().map_err(|error| {
            increment(&mut self.counts.refused_producers);
            RegisterError::Entropy(error)
        })?;
        let (lifecycle, lifetime) = retirement::Lifecycle::pair();
        let sender = self.register_identity(owner, identity, lifecycle, lineage)?;
        Ok(Opened { sender, lifetime })
    }

    fn register_identity(
        &mut self,
        owner: Owner,
        identity: Id,
        lifecycle: retirement::Lifecycle,
        lineage: Lineage,
    ) -> Result<Sender, RegisterError> {
        self.validate_registration(identity)?;
        let (channel, sender) = Channel::pair().map_err(|error| {
            increment(&mut self.counts.refused_producers);
            RegisterError::Transport(error)
        })?;
        self.channels.push(Producer {
            owner,
            lineage,
            identity,
            channel,
            delivery: Delivery::default(),
            lifecycle,
            _lease: None,
        });
        Ok(Sender::new(sender, owner, identity).with_lineage(lineage))
    }

    #[cfg(test)]
    fn register(&mut self, owner: Owner, identity: Id) -> Result<Sender, RegisterError> {
        self.register_identity(
            owner,
            identity,
            retirement::Lifecycle::new(),
            Lineage::default(),
        )
    }

    fn validate_registration(&mut self, identity: Id) -> Result<(), RegisterError> {
        let error = if identity == self.local.producer
            || self
                .channels
                .iter()
                .any(|channel| channel.identity == identity)
        {
            Some(RegisterError::Duplicate)
        } else if self.channels.len() + 1 >= MAX_PRODUCERS {
            Some(RegisterError::Capacity)
        } else {
            None
        };
        if let Some(error) = error {
            increment(&mut self.counts.refused_producers);
            return Err(error);
        }
        Ok(())
    }

    fn admit(&mut self, registration: worker::Registration) {
        let worker::Registration {
            owner,
            lineage,
            identity,
            channel,
            lease,
            lifecycle,
        } = registration;
        if self.validate_registration(identity).is_err() {
            return;
        }
        self.channels.push(Producer {
            owner,
            lineage,
            identity,
            channel,
            delivery: Delivery::default(),
            lifecycle,
            _lease: Some(lease),
        });
    }

    pub fn poll(&mut self) {
        let now = Instant::now();
        self.poll_at(now);
        self.retire(now);
    }

    fn poll_at(&mut self, now: Instant) {
        self.poll_while(now, || true);
    }

    fn poll_while(&mut self, now: Instant, mut running: impl FnMut() -> bool) {
        self.expire(now);
        if !running() {
            return;
        }
        if let Ok(bytes) = self.local.try_recv() {
            self.accept(&bytes, Owner::Magi, self.local.producer, now);
        }
        let mut frame = [0; MAX_EVENT_BYTES];
        for index in 0..self.channels.len() {
            if !running() {
                break;
            }
            let Some(producer) = self.channels.get(index) else {
                break;
            };
            let owner = producer.owner;
            let identity = producer.identity;
            match producer.channel.receive(&mut frame) {
                Received::Frame(n) => {
                    if let Some(bytes) = frame.get(..n) {
                        self.accept(bytes, owner, identity, now);
                    }
                }
                Received::Empty => {}
                Received::Invalid => self.reject(identity),
                Received::Unavailable => {
                    increment(&mut self.counts.unavailable);
                    if let Some(producer) = self.channels.get_mut(index) {
                        producer.delivery.transport_fault = true;
                    }
                }
            }
        }
    }

    fn retire(&mut self, now: Instant) {
        let mut index = 0;
        while index < self.channels.len() {
            let Some(producer) = self.channels.get_mut(index) else {
                break;
            };
            let ending = producer.lifecycle.ending(now, producer.delivery.finished());
            let empty = ending && producer.channel.is_empty();
            if ending && (producer.lifecycle.expired(now) || empty) {
                if !empty {
                    producer.delivery.transport_fault = true;
                }
                let producer = self.channels.swap_remove(index);
                self.retired.record(producer.delivery);
            } else {
                index += 1;
            }
        }
    }

    fn accept(&mut self, bytes: &[u8], owner: Owner, producer: Id, now: Instant) {
        let Ok(event) = wire::decode(bytes, owner, producer) else {
            if let Some(status) = status::decode(bytes, owner, producer) {
                self.accept_status(producer, status);
            } else {
                self.reject(producer);
            }
            return;
        };
        let lineage = if producer == self.local.producer {
            self.local.lineage
        } else {
            self.channels
                .iter()
                .find(|channel| channel.identity == producer)
                .map_or_else(Lineage::default, |channel| channel.lineage)
        };
        if !lineage.accepts(event.correlation) {
            self.reject(producer);
            return;
        }
        if let Some(channel) = self
            .channels
            .iter_mut()
            .find(|channel| channel.identity == producer)
        {
            if channel.delivery.finished() {
                self.reject(producer);
                return;
            }
            increment(&mut channel.delivery.received_events);
        }
        let key = (event.producer, event.sequence);
        if self.seen.contains_key(&key) {
            increment(&mut self.counts.duplicate);
            return;
        }
        if self.seen.len() == MAX_SEEN {
            self.evict_seen();
        }
        self.seen.insert(key, now);
        self.order.push_back((key, now));
        increment(&mut self.counts.accepted);
        if let Some(usage) = event.usage {
            let Some(ask) = event.correlation.ask_id else {
                increment(&mut self.counts.usage_unattributed);
                return;
            };
            self.update_usage(event.producer, ask, usage, now);
        }
    }

    fn update_usage(&mut self, producer: Id, ask: Id, usage: Usage, now: Instant) {
        if let Some(index) = self
            .usage
            .iter()
            .position(|entry| entry.producer == producer && entry.ask == ask)
        {
            let Some(prior) = self.usage.get(index) else {
                return;
            };
            if usage.revision < prior.usage.revision {
                increment(&mut self.counts.usage_stale);
                return;
            }
            if usage.revision == prior.usage.revision {
                if usage != prior.usage {
                    increment(&mut self.counts.usage_conflicts);
                }
                return;
            }
            self.usage.remove(index);
        } else if self.usage.len() == MAX_ASKS {
            self.usage.pop_front();
            increment(&mut self.counts.usage_evicted);
        }
        self.usage.push_back(Snapshot {
            producer,
            ask,
            usage,
            at: now,
        });
    }

    fn expire(&mut self, now: Instant) {
        while self
            .order
            .front()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= RETENTION)
        {
            self.evict_seen();
        }
        while self
            .usage
            .front()
            .is_some_and(|entry| now.saturating_duration_since(entry.at) >= RETENTION)
        {
            self.usage.pop_front();
            increment(&mut self.counts.usage_evicted);
        }
    }

    fn evict_seen(&mut self) {
        if let Some((key, _)) = self.order.pop_front() {
            self.seen.remove(&key);
            increment(&mut self.counts.dedup_evicted);
        }
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    fn reject(&mut self, producer: Id) {
        increment(&mut self.counts.invalid);
        if let Some(channel) = self
            .channels
            .iter_mut()
            .find(|channel| channel.identity == producer)
        {
            increment(&mut channel.delivery.invalid_frames);
        }
    }

    fn accept_status(&mut self, producer: Id, next: ProducerStatus) {
        let Some(channel) = self
            .channels
            .iter_mut()
            .find(|channel| channel.identity == producer)
        else {
            self.reject(producer);
            return;
        };
        match channel.delivery.update(next) {
            status::Updated::Accepted => increment(&mut self.counts.statuses),
            status::Updated::Stale => increment(&mut self.counts.status_stale),
            status::Updated::Invalid => self.reject(producer),
        }
    }

    pub fn delivery(&self, producer: Id) -> Option<Delivery> {
        self.channels
            .iter()
            .find(|channel| channel.identity == producer)
            .map(|channel| channel.delivery)
    }

    pub fn summary(&self) -> Summary {
        let final_confirmed = self
            .channels
            .iter()
            .filter(|channel| channel.delivery.final_confirmed())
            .count();
        let reported_loss_producers = self
            .channels
            .iter()
            .filter(|channel| channel.delivery.has_reported_loss())
            .count();
        Summary {
            counts: self.counts,
            local_loss: self.local_loss(),
            remote_producers: self.channels.len(),
            final_confirmed,
            unknown_loss_producers: self.channels.len() - final_confirmed,
            reported_loss_producers,
            retired: self.retired,
            closed: false,
            local_unconfirmed: true,
            worker_failed: false,
            shutdown_overrun: false,
        }
    }

    pub fn close(self) -> Summary {
        let mut summary = self.summary();
        let counters = self.local.loss.clone();
        drop(self);
        summary.local_unconfirmed = counters.active.load(std::sync::atomic::Ordering::SeqCst) != 0;
        summary.local_loss = counters.snapshot();
        summary.closed = true;
        summary
    }

    pub fn local_loss(&self) -> super::Loss {
        self.local.loss.snapshot()
    }

    pub fn usage(&self, producer: Id, ask: Id) -> Option<Usage> {
        self.usage
            .iter()
            .find(|entry| entry.producer == producer && entry.ask == ask)
            .map(|entry| entry.usage)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Summary {
    pub counts: Counts,
    pub local_loss: super::Loss,
    pub remote_producers: usize,
    pub final_confirmed: usize,
    pub unknown_loss_producers: usize,
    pub reported_loss_producers: usize,
    pub retired: Retired,
    pub closed: bool,
    pub local_unconfirmed: bool,
    pub worker_failed: bool,
    pub shutdown_overrun: bool,
}

impl Summary {
    pub fn incomplete(&self) -> bool {
        !self.closed
            || self.worker_failed
            || self.shutdown_overrun
            || self.local_unconfirmed
            || self.unknown_loss_producers != 0
            || self.reported_loss_producers != 0
            || self.retired.unknown_loss_producers != 0
            || self.retired.reported_loss_producers != 0
            || self.local_loss != super::Loss::default()
            || self.counts.invalid != 0
            || self.counts.unavailable != 0
            || self.counts.refused_producers != 0
            || self.counts.dedup_evicted != 0
            || self.counts.usage_evicted != 0
            || self.counts.usage_unattributed != 0
            || self.counts.usage_conflicts != 0
    }
}

fn increment(counter: &mut u64) {
    *counter = counter.saturating_add(1);
}

#[cfg(test)]
mod tests;
