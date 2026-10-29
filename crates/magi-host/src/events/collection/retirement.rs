use super::{Delivery, Sender, TransportLoss};
use crate::events::Loss;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub const RETIREMENT_BUDGET: Duration = Duration::from_secs(1);

pub struct Opened {
    pub sender: Sender,
    pub lifetime: Lifetime,
}

/// Keep with the native owner; dropping requests bounded collection retirement.
pub struct Lifetime(Arc<AtomicBool>);

impl Drop for Lifetime {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) struct Lifecycle {
    ended: Arc<AtomicBool>,
    deadline: Option<Instant>,
}

impl Lifecycle {
    pub(super) fn new() -> Self {
        Self {
            ended: Arc::new(AtomicBool::new(false)),
            deadline: None,
        }
    }

    pub(super) fn pair() -> (Self, Lifetime) {
        let lifecycle = Self::new();
        let lifetime = Lifetime(lifecycle.ended.clone());
        (lifecycle, lifetime)
    }

    pub(super) fn ending(&mut self, now: Instant, finished: bool) -> bool {
        if self.deadline.is_none() && (finished || self.ended.load(Ordering::Acquire)) {
            self.deadline = now.checked_add(RETIREMENT_BUDGET);
        }
        self.deadline.is_some()
    }

    pub(super) fn expired(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Retired {
    pub producers: u64,
    pub final_confirmed: u64,
    pub unknown_loss_producers: u64,
    pub reported_loss_producers: u64,
    pub queue_loss: Loss,
    pub transport_loss: TransportLoss,
}

impl Retired {
    pub(super) fn record(&mut self, delivery: Delivery) {
        self.producers = self.producers.saturating_add(1);
        let counter = if delivery.final_confirmed() {
            &mut self.final_confirmed
        } else {
            &mut self.unknown_loss_producers
        };
        *counter = counter.saturating_add(1);
        if delivery.has_reported_loss() {
            self.reported_loss_producers = self.reported_loss_producers.saturating_add(1);
        }
        if let Some(status) = delivery.last_status {
            macro_rules! add {
                ($field:ident, $($counter:ident),+ $(,)?) => {
                    $(self.$field.$counter = self.$field.$counter.saturating_add(status.$field.$counter);)+
                };
            }
            add!(queue_loss, abandoned, full, closed, encoding, exhausted);
            add!(transport_loss, full, unavailable, invalid);
        }
    }
}

#[cfg(test)]
mod tests;
