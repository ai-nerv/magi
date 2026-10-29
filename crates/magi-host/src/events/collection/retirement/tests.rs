#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::super::{Collector, Id, MAX_PRODUCERS, Owner, Report, Sent, Worker};
use super::*;
use crate::events::Emitter;
use serde_json::json;
use std::{collections::HashSet, num::NonZeroU64, thread};

fn setup() -> Collector {
    let (_, receiver) = Emitter::new(Id::generate().expect("identity"), [1, 2, 3]);
    Collector::new(receiver)
}

fn event(identity: Id, sequence: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema":1, "program":"magi", "program_version":[1,2,3],
        "producer":String::from(identity), "sequence":sequence, "correlation":{},
        "fact":{"kind":"client", "outcome":"connected"}
    }))
    .expect("event")
}

#[test]
fn sequential_producers_reuse_capacity_without_retaining_identity_history() {
    let mut collector = setup();
    let mut identities = HashSet::new();
    for n in 1..=MAX_PRODUCERS * 3 {
        let Opened {
            mut sender,
            lifetime,
        } = collector.open(Owner::Magi).expect("producer");
        assert!(identities.insert(sender.identity()));
        assert_eq!(sender.send(&event(sender.identity(), 1)), Sent::Queued);
        assert_eq!(
            sender.report(Loss::default(), true),
            Report::Sent(Sent::Queued)
        );
        collector.poll();
        assert_eq!(collector.summary().remote_producers, 1);
        collector.poll();
        let summary = collector.summary();
        assert_eq!(summary.remote_producers, 0);
        assert_eq!(summary.retired.producers, n as u64);
        assert_eq!(summary.retired.final_confirmed, n as u64);
        assert_eq!(summary.retired.unknown_loss_producers, 0);
        assert!(collector.channels.is_empty());
        drop(lifetime);
    }
    assert_eq!(collector.counts().accepted, (MAX_PRODUCERS * 3) as u64);
    assert!(!collector.close().incomplete());
}

#[test]
fn owner_exit_drains_queued_frames_and_final_status_without_an_acknowledgment() {
    let mut collector = setup();
    let Opened {
        mut sender,
        lifetime,
    } = collector.open(Owner::Magi).expect("producer");
    for n in 1..=3 {
        assert_eq!(sender.send(&event(sender.identity(), n)), Sent::Queued);
    }
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    let started = Instant::now();
    drop(lifetime);
    assert!(started.elapsed() < RETIREMENT_BUDGET);
    for n in 1..=3 {
        collector.poll();
        assert_eq!(collector.counts().accepted, n);
        assert_eq!(collector.summary().remote_producers, 1);
    }
    collector.poll();
    let summary = collector.close();
    assert_eq!(summary.retired.final_confirmed, 1);
    assert!(!summary.incomplete());
}

#[test]
fn missing_final_status_and_known_losses_survive_slot_reuse() {
    let mut collector = setup();
    let Opened {
        mut sender,
        lifetime,
    } = collector.open(Owner::Magi).expect("producer");
    assert_eq!(sender.send(b""), Sent::Invalid);
    let loss = Loss {
        full: 7,
        abandoned: 3,
        ..Loss::default()
    };
    assert_eq!(sender.report(loss, false), Report::Sent(Sent::Queued));
    collector.poll();
    drop(lifetime);
    collector.poll();
    assert_eq!(sender.send(&event(sender.identity(), 1)), Sent::Unavailable);
    let Opened {
        mut sender,
        lifetime,
    } = collector.open(Owner::Magi).expect("replacement");
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    collector.poll();
    drop(lifetime);
    let summary = collector.close();
    assert_eq!(summary.retired.producers, 2);
    assert_eq!(summary.retired.final_confirmed, 1);
    assert_eq!(summary.retired.unknown_loss_producers, 1);
    assert_eq!(summary.retired.reported_loss_producers, 1);
    assert_eq!(summary.retired.queue_loss, loss);
    assert_eq!(summary.retired.transport_loss.invalid, 1);
    assert!(summary.incomplete());
}

#[test]
fn stale_lifetime_cannot_close_a_reused_slot() {
    let mut collector = setup();
    let Opened {
        mut sender,
        lifetime,
    } = collector.open(Owner::Magi).expect("first");
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    collector.poll();
    let Opened {
        mut sender,
        lifetime: next_lifetime,
    } = collector.open(Owner::Magi).expect("next");
    drop(lifetime);
    collector.poll();
    assert_eq!(collector.summary().remote_producers, 1);
    assert_eq!(sender.send(&event(sender.identity(), 1)), Sent::Queued);
    collector.poll();
    assert_eq!(collector.counts().accepted, 1);
    drop(next_lifetime);
    collector.poll();
    assert_eq!(collector.close().retired.unknown_loss_producers, 1);
}

#[test]
fn retirement_deadline_abandons_a_nonempty_noisy_producer() {
    let mut collector = setup();
    let Opened {
        mut sender,
        lifetime,
    } = collector.open(Owner::Magi).expect("producer");
    let now = Instant::now();
    assert_eq!(sender.send(&event(sender.identity(), 1)), Sent::Queued);
    drop(lifetime);
    collector.retire(now);
    collector.retire(now + RETIREMENT_BUDGET - Duration::from_nanos(1));
    assert_eq!(collector.summary().remote_producers, 1);
    collector.retire(now + RETIREMENT_BUDGET);
    assert_eq!(collector.summary().remote_producers, 0);
    assert_eq!(collector.summary().retired.unknown_loss_producers, 1);
    assert_eq!(collector.counts().accepted, 0);
    assert_eq!(sender.send(&event(sender.identity(), 2)), Sent::Unavailable);
}

#[test]
fn queued_traffic_after_final_prevents_false_complete_retirement() {
    let mut collector = setup();
    let Opened {
        mut sender,
        lifetime,
    } = collector.open(Owner::Magi).expect("producer");
    let status = serde_json::to_vec(&json!({
        "schema":1, "program":"magi", "producer":String::from(sender.identity()),
        "status":{"revision":1, "events_sent":0, "finished":true,
            "queue_loss":Loss::default(), "transport_loss":TransportLoss::default()}
    }))
    .expect("raw final status");
    assert_eq!(sender.send(&status), Sent::Queued);
    assert_eq!(sender.send(&event(sender.identity(), 1)), Sent::Queued);
    collector.poll();
    assert_eq!(collector.summary().remote_producers, 1);
    collector.poll();
    assert_eq!(collector.counts().invalid, 1);
    let summary = collector.close();
    assert_eq!(summary.retired.unknown_loss_producers, 1);
    assert_eq!(summary.retired.final_confirmed, 0);
    assert!(summary.incomplete());
    drop(lifetime);
}

#[test]
fn retirement_totals_saturate_instead_of_wrapping_or_losing_known_drops() {
    let mut retired = Retired {
        producers: u64::MAX,
        final_confirmed: u64::MAX,
        unknown_loss_producers: u64::MAX,
        reported_loss_producers: u64::MAX,
        queue_loss: Loss {
            abandoned: u64::MAX,
            full: u64::MAX,
            closed: u64::MAX,
            encoding: u64::MAX,
            exhausted: u64::MAX,
        },
        transport_loss: TransportLoss {
            full: u64::MAX,
            unavailable: u64::MAX,
            invalid: u64::MAX,
        },
    };
    let initial = retired;
    let mut delivery = Delivery {
        last_status: Some(super::super::ProducerStatus {
            revision: NonZeroU64::new(1).expect("revision"),
            events_sent: 0,
            finished: true,
            queue_loss: retired.queue_loss,
            transport_loss: retired.transport_loss,
        }),
        ..Delivery::default()
    };
    retired.record(delivery);
    delivery.transport_fault = true;
    retired.record(delivery);
    assert_eq!(retired, initial);
}

fn wait(worker: &Worker, retired: u64) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while worker.readout().snapshot().retired.producers < retired {
        assert!(Instant::now() < deadline, "retirement deadline");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn worker_releases_preexisting_and_new_registration_leases() {
    let mut collector = setup();
    let first = collector.open(Owner::Magi).expect("preexisting");
    let worker = Worker::start(collector).expect("worker");
    drop(first.lifetime);
    wait(&worker, 1);
    assert_eq!(worker.readout().snapshot().remote_producers, 0);
    let mut opened = Vec::new();
    for _ in 1..MAX_PRODUCERS {
        opened.push(worker.open(Owner::Magi).expect("full concurrent capacity"));
    }
    assert!(matches!(
        worker.open(Owner::Magi),
        Err(super::super::RegisterError::Capacity)
    ));
    drop(opened);
    wait(&worker, MAX_PRODUCERS as u64);
    for n in 1..=MAX_PRODUCERS * 2 {
        let Opened {
            mut sender,
            lifetime,
        } = worker.open(Owner::Magi).expect("reuse");
        assert_eq!(
            sender.report(Loss::default(), true),
            Report::Sent(Sent::Queued)
        );
        drop(lifetime);
        wait(&worker, (MAX_PRODUCERS + n) as u64);
    }
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.remote_producers, 0);
    assert_eq!(summary.retired.producers, (MAX_PRODUCERS * 3) as u64);
    assert_eq!(summary.retired.unknown_loss_producers, MAX_PRODUCERS as u64);
    assert_eq!(summary.retired.final_confirmed, (MAX_PRODUCERS * 2) as u64);
    assert!(summary.incomplete());
}
