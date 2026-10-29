use super::super::{Encoded, Id, Loss, MAX_EVENT_BYTES, Revision};
use super::{Owner, Sent, TransportLoss, transport};
use serde::{Deserialize, Serialize};
use std::{
    num::NonZeroU64,
    time::{Duration, Instant},
};

pub const REPORT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerStatus {
    pub revision: NonZeroU64,
    pub events_sent: u64,
    pub queue_loss: Loss,
    pub transport_loss: TransportLoss,
    pub finished: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    schema: Revision,
    program: Owner,
    producer: Id,
    status: ProducerStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    Sent(Sent),
    RateLimited,
    Exhausted,
    Closed,
    Encoding,
}

pub struct Sender {
    inner: transport::Sender,
    lineage: crate::events::Lineage,
    owner: Owner,
    producer: Id,
    events_sent: u64,
    next_revision: u64,
    last_report: Option<Instant>,
    finished: bool,
}

impl Sender {
    pub fn lineage(&self) -> crate::events::Lineage {
        self.lineage
    }

    pub(super) fn with_lineage(mut self, lineage: crate::events::Lineage) -> Self {
        self.lineage = lineage;
        self
    }

    pub(super) fn into_fresh(self) -> Option<(transport::Sender, Owner, Id)> {
        (self.events_sent == 0
            && self.next_revision == 1
            && self.last_report.is_none()
            && !self.finished
            && self.inner.loss() == TransportLoss::default())
        .then_some((self.inner, self.owner, self.producer))
    }

    pub fn identity(&self) -> Id {
        self.producer
    }

    pub(super) fn new(inner: transport::Sender, owner: Owner, producer: Id) -> Self {
        Self {
            inner,
            lineage: crate::events::Lineage::default(),
            owner,
            producer,
            events_sent: 0,
            next_revision: 1,
            last_report: None,
            finished: false,
        }
    }

    pub fn send(&mut self, bytes: &[u8]) -> Sent {
        if self.finished || self.events_sent == u64::MAX {
            return self.inner.send(&[]);
        }
        let result = self.inner.send(bytes);
        if result == Sent::Queued {
            self.events_sent += 1;
        }
        result
    }

    pub fn loss(&self) -> TransportLoss {
        self.inner.loss()
    }

    pub fn report(&mut self, queue_loss: Loss, finished: bool) -> Report {
        self.report_at(queue_loss, finished, Instant::now())
    }

    fn report_at(&mut self, queue_loss: Loss, finished: bool, now: Instant) -> Report {
        if self.finished {
            return Report::Closed;
        }
        if self
            .last_report
            .is_some_and(|last| now.saturating_duration_since(last) < REPORT_INTERVAL)
        {
            return Report::RateLimited;
        }
        let Some(next_revision) = self.next_revision.checked_add(1) else {
            return Report::Exhausted;
        };
        let Some(revision) = NonZeroU64::new(self.next_revision) else {
            return Report::Exhausted;
        };
        self.last_report = Some(now);
        self.next_revision = next_revision;
        let frame = Frame {
            schema: Revision,
            program: self.owner,
            producer: self.producer,
            status: ProducerStatus {
                revision,
                events_sent: self.events_sent,
                queue_loss,
                transport_loss: self.inner.loss(),
                finished,
            },
        };
        let mut encoded = Encoded {
            bytes: [0; MAX_EVENT_BYTES],
            len: 0,
        };
        if serde_json::to_writer(&mut encoded, &frame).is_err() {
            return Report::Encoding;
        }
        let Some(bytes) = encoded.bytes.get(..encoded.len) else {
            return Report::Encoding;
        };
        let result = self.inner.send(bytes);
        if result == Sent::Queued && finished {
            self.finished = true;
        }
        Report::Sent(result)
    }
}

pub(super) fn decode(bytes: &[u8], owner: Owner, producer: Id) -> Option<ProducerStatus> {
    if bytes.len() > MAX_EVENT_BYTES {
        return None;
    }
    let frame: Frame = serde_json::from_slice(bytes).ok()?;
    (frame.program == owner && frame.producer == producer).then_some(frame.status)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    pub last_status: Option<ProducerStatus>,
    pub received_events: u64,
    pub invalid_frames: u64,
    pub transport_fault: bool,
}

impl Delivery {
    pub fn final_confirmed(&self) -> bool {
        self.invalid_frames == 0
            && !self.transport_fault
            && self
                .last_status
                .is_some_and(|status| status.finished && status.events_sent == self.received_events)
    }

    pub fn has_reported_loss(&self) -> bool {
        self.last_status.is_some_and(|status| {
            status.queue_loss != Loss::default()
                || status.transport_loss != TransportLoss::default()
        })
    }

    pub(super) fn finished(&self) -> bool {
        self.last_status.is_some_and(|status| status.finished)
    }

    pub(super) fn update(&mut self, next: ProducerStatus) -> Updated {
        if let Some(prior) = self.last_status {
            if next.revision < prior.revision || next == prior {
                return Updated::Stale;
            }
            if prior.finished || next.revision == prior.revision || !monotonic(prior, next) {
                return Updated::Invalid;
            }
        }
        if next.events_sent < self.received_events {
            return Updated::Invalid;
        }
        self.last_status = Some(next);
        Updated::Accepted
    }
}

pub(super) enum Updated {
    Accepted,
    Stale,
    Invalid,
}

fn monotonic(prior: ProducerStatus, next: ProducerStatus) -> bool {
    fn counters(status: ProducerStatus) -> [u64; 9] {
        [
            status.events_sent,
            status.queue_loss.abandoned,
            status.queue_loss.full,
            status.queue_loss.closed,
            status.queue_loss.encoding,
            status.queue_loss.exhausted,
            status.transport_loss.full,
            status.transport_loss.unavailable,
            status.transport_loss.invalid,
        ]
    }
    counters(prior)
        .into_iter()
        .zip(counters(next))
        .all(|(old, new)| new >= old)
}

#[cfg(test)]
mod tests;
