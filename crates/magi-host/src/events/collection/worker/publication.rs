use super::super::super::Loss;
use super::super::{Counts, Retired, Summary, TransportLoss};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

macro_rules! counters {
    ($cell:ident, $value:ident, $($field:ident),+ $(,)?) => {
        #[derive(Default)]
        struct $cell { $($field: AtomicU64,)+ }
        impl $cell {
            fn store(&self, value: $value) { $(self.$field.store(value.$field, Ordering::Relaxed);)+ }
            fn load(&self) -> $value { $value { $($field: self.$field.load(Ordering::Relaxed),)+ } }
        }
    };
}

counters!(
    CountAtoms,
    Counts,
    accepted,
    duplicate,
    invalid,
    unavailable,
    refused_producers,
    dedup_evicted,
    usage_evicted,
    usage_unattributed,
    usage_stale,
    usage_conflicts,
    statuses,
    status_stale
);
counters!(
    LossAtoms, Loss, abandoned, full, closed, encoding, exhausted
);
counters!(TransportAtoms, TransportLoss, full, unavailable, invalid);

#[derive(Default)]
pub(super) struct Published {
    counts: CountAtoms,
    loss: LossAtoms,
    remote_producers: AtomicUsize,
    final_confirmed: AtomicUsize,
    unknown_loss_producers: AtomicUsize,
    reported_loss_producers: AtomicUsize,
    retired_producers: AtomicU64,
    retired_confirmed: AtomicU64,
    retired_unknown: AtomicU64,
    retired_reported: AtomicU64,
    retired_queue: LossAtoms,
    retired_transport: TransportAtoms,
    local_unconfirmed: AtomicBool,
    worker_failed: AtomicBool,
    shutdown_overrun: AtomicBool,
    closed: AtomicBool,
}

impl Published {
    pub(super) fn fail(&self) {
        self.worker_failed.store(true, Ordering::Release);
    }

    pub(super) fn store(&self, value: Summary) {
        self.counts.store(value.counts);
        self.loss.store(value.local_loss);
        self.remote_producers
            .store(value.remote_producers, Ordering::Relaxed);
        self.final_confirmed
            .store(value.final_confirmed, Ordering::Relaxed);
        self.unknown_loss_producers
            .store(value.unknown_loss_producers, Ordering::Relaxed);
        self.reported_loss_producers
            .store(value.reported_loss_producers, Ordering::Relaxed);
        self.retired_producers
            .store(value.retired.producers, Ordering::Relaxed);
        self.retired_confirmed
            .store(value.retired.final_confirmed, Ordering::Relaxed);
        self.retired_unknown
            .store(value.retired.unknown_loss_producers, Ordering::Relaxed);
        self.retired_reported
            .store(value.retired.reported_loss_producers, Ordering::Relaxed);
        self.retired_queue.store(value.retired.queue_loss);
        self.retired_transport.store(value.retired.transport_loss);
        self.local_unconfirmed
            .store(value.local_unconfirmed, Ordering::Relaxed);
        self.worker_failed
            .store(value.worker_failed, Ordering::Relaxed);
        self.shutdown_overrun
            .store(value.shutdown_overrun, Ordering::Relaxed);
        self.closed.store(value.closed, Ordering::Release);
    }

    pub(super) fn load(&self) -> Summary {
        let closed = self.closed.load(Ordering::Acquire);
        Summary {
            counts: self.counts.load(),
            local_loss: self.loss.load(),
            remote_producers: self.remote_producers.load(Ordering::Relaxed),
            final_confirmed: self.final_confirmed.load(Ordering::Relaxed),
            unknown_loss_producers: self.unknown_loss_producers.load(Ordering::Relaxed),
            reported_loss_producers: self.reported_loss_producers.load(Ordering::Relaxed),
            retired: Retired {
                producers: self.retired_producers.load(Ordering::Relaxed),
                final_confirmed: self.retired_confirmed.load(Ordering::Relaxed),
                unknown_loss_producers: self.retired_unknown.load(Ordering::Relaxed),
                reported_loss_producers: self.retired_reported.load(Ordering::Relaxed),
                queue_loss: self.retired_queue.load(),
                transport_loss: self.retired_transport.load(),
            },
            local_unconfirmed: self.local_unconfirmed.load(Ordering::Relaxed),
            worker_failed: self.worker_failed.load(Ordering::Acquire),
            shutdown_overrun: self.shutdown_overrun.load(Ordering::Relaxed),
            closed,
        }
    }
}
