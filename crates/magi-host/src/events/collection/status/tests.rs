#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::super::super::{ClientOutcome, Correlation, Emitted, Emitter, Fact};
use super::super::{
    Collector, Counts,
    transport::{Channel, Received},
};
use super::*;
use serde_json::{Value, json};

const GOLDEN: &str = include_str!("golden.json");

fn id(n: u64) -> Id {
    Id::try_from(format!("{n:032x}")).expect("id")
}

fn setup() -> (Emitter, Collector, Sender) {
    let (emitter, receiver) = Emitter::new(id(1), [1, 2, 3]);
    let mut collector = Collector::new(receiver);
    let sender = collector
        .register(Owner::Magi, id(2))
        .expect("registration");
    (emitter, collector, sender)
}

fn event() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema":1, "program":"magi", "program_version":[1,2,3],
        "producer":format!("{:032x}",2), "sequence":1, "correlation":{},
        "fact":{"kind":"client", "outcome":"connected"}
    }))
    .expect("event")
}

fn status(revision: u64, events_sent: u64, finished: bool) -> ProducerStatus {
    ProducerStatus {
        revision: NonZeroU64::new(revision).expect("nonzero"),
        events_sent,
        queue_loss: Loss::default(),
        transport_loss: TransportLoss::default(),
        finished,
    }
}

fn raw(sender: &mut Sender, status: ProducerStatus) {
    let frame = Frame {
        schema: Revision,
        program: Owner::Magi,
        producer: id(2),
        status,
    };
    assert_eq!(
        sender
            .inner
            .send(&serde_json::to_vec(&frame).expect("status")),
        Sent::Queued
    );
}

#[test]
fn independent_status_golden_is_exact_and_bound_to_channel() {
    let (channel, transport) = Channel::pair().expect("pair");
    let mut sender = Sender::new(transport, Owner::Magi, id(2));
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    let mut frame = [0; MAX_EVENT_BYTES];
    let Received::Frame(n) = channel.receive(&mut frame) else {
        panic!("status frame")
    };
    assert_eq!(&frame[..n], GOLDEN.trim_end().as_bytes());
    let value = decode(&frame[..n], Owner::Magi, id(2)).expect("strict status");
    assert_eq!(value, status(1, 0, true));
    assert!(decode(&frame[..n], Owner::Casper, id(2)).is_none());
    assert!(decode(&frame[..n], Owner::Magi, id(3)).is_none());
    assert_eq!(sender.report(Loss::default(), true), Report::Closed);
    assert_eq!(sender.send(&event()), Sent::Invalid);
    assert_eq!(channel.receive(&mut frame), Received::Empty);
}

#[test]
fn status_schema_rejects_content_duplicate_fields_and_invalid_numbers() {
    let original: Value = serde_json::from_str(GOLDEN).expect("golden");
    for path in [
        "",
        "/status",
        "/status/queue_loss",
        "/status/transport_loss",
    ] {
        let mut value = original.clone();
        value.pointer_mut(path).expect("object")["secret"] = json!("private");
        assert!(
            decode(
                &serde_json::to_vec(&value).expect("JSON"),
                Owner::Magi,
                id(2)
            )
            .is_none()
        );
    }
    for (path, replacement) in [
        ("/schema", json!(2)),
        ("/status/revision", json!(0)),
        ("/status/events_sent", json!(-1)),
        ("/status/finished", json!("true")),
        ("/status/queue_loss/full", json!(1.5)),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(path).expect("field") = replacement;
        assert!(
            decode(
                &serde_json::to_vec(&value).expect("JSON"),
                Owner::Magi,
                id(2)
            )
            .is_none()
        );
    }
    let duplicate = GOLDEN.replace("\"revision\":1", "\"revision\":1,\"revision\":1");
    assert!(decode(duplicate.as_bytes(), Owner::Magi, id(2)).is_none());
    assert!(decode(&[b' '; MAX_EVENT_BYTES + 1], Owner::Magi, id(2)).is_none());
}

#[test]
fn reports_are_rate_limited_including_failed_attempts_and_final_status() {
    let (channel, transport) = Channel::pair().expect("pair");
    let mut sender = Sender::new(transport, Owner::Magi, id(2));
    let now = Instant::now();
    let mut full = false;
    for _ in 0..10000 {
        if sender.send(&[b'x'; MAX_EVENT_BYTES]) == Sent::Full {
            full = true;
            break;
        }
    }
    assert!(full);
    assert_eq!(
        sender.report_at(Loss::default(), false, now),
        Report::Sent(Sent::Full)
    );
    let loss = sender.loss();
    assert_eq!(
        sender.report_at(Loss::default(), true, now + Duration::from_millis(999)),
        Report::RateLimited
    );
    assert_eq!(sender.loss(), loss);
    let mut frame = [0; MAX_EVENT_BYTES];
    while matches!(channel.receive(&mut frame), Received::Frame(_)) {}
    assert_eq!(
        sender.report_at(Loss::default(), true, now + REPORT_INTERVAL),
        Report::Sent(Sent::Queued)
    );
    let Received::Frame(n) = channel.receive(&mut frame) else {
        panic!("final status")
    };
    let value = decode(&frame[..n], Owner::Magi, id(2)).expect("status");
    assert_eq!(value.revision.get(), 2);
    assert_eq!(value.transport_loss, loss);
    assert!(value.finished);
}

#[test]
fn final_status_confirms_only_received_frames_and_preserves_reported_losses() {
    let (_, mut collector, mut sender) = setup();
    assert_eq!(sender.send(&event()), Sent::Queued);
    let queue_loss = Loss {
        full: 7,
        abandoned: 2,
        ..Loss::default()
    };
    assert_eq!(sender.report(queue_loss, true), Report::Sent(Sent::Queued));
    collector.poll_at(Instant::now());
    assert!(
        !collector
            .delivery(id(2))
            .expect("delivery")
            .final_confirmed()
    );
    collector.poll_at(Instant::now());
    let delivery = collector.delivery(id(2)).expect("delivery");
    assert!(delivery.final_confirmed());
    assert!(delivery.has_reported_loss());
    assert_eq!(delivery.received_events, 1);
    assert_eq!(delivery.last_status.expect("status").queue_loss, queue_loss);
    assert_eq!(collector.counts().accepted, 1);
    assert_eq!(collector.counts().statuses, 1);
    let summary = collector.close();
    assert_eq!(summary.unknown_loss_producers, 0);
    assert_eq!(summary.reported_loss_producers, 1);
    assert!(summary.incomplete());
}

#[test]
fn orderly_empty_close_is_distinct_from_a_missing_final_report() {
    let (_, mut collector, mut sender) = setup();
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    collector.poll_at(Instant::now());
    assert!(collector.summary().incomplete());
    let summary = collector.close();
    assert!(!summary.incomplete());
    assert!(summary.closed);
    let (_, collector, sender) = setup();
    drop(sender);
    let summary = collector.close();
    assert_eq!(summary.unknown_loss_producers, 1);
    assert!(summary.incomplete());
}

#[test]
fn missing_events_and_invalid_frames_cannot_be_hidden_by_a_final_status() {
    let (_, mut collector, mut sender) = setup();
    raw(&mut sender, status(1, 1, true));
    collector.poll_at(Instant::now());
    assert!(
        !collector
            .delivery(id(2))
            .expect("delivery")
            .final_confirmed()
    );
    assert_eq!(collector.close().unknown_loss_producers, 1);
    let (_, mut collector, mut sender) = setup();
    assert_eq!(sender.send(b"malformed"), Sent::Queued);
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    collector.poll_at(Instant::now());
    collector.poll_at(Instant::now());
    let delivery = collector.delivery(id(2)).expect("delivery");
    assert_eq!(delivery.invalid_frames, 1);
    assert!(!delivery.final_confirmed());
}

#[test]
fn stale_identical_reports_are_ignored_but_conflicts_and_regressions_are_invalid() {
    let (_, mut collector, mut sender) = setup();
    let mut first = status(2, 0, false);
    first.queue_loss.full = 3;
    for value in [
        first,
        first,
        status(1, 0, false),
        status(2, 0, true),
        status(3, 0, false),
    ] {
        raw(&mut sender, value);
        collector.poll_at(Instant::now());
    }
    assert_eq!(
        collector.delivery(id(2)).expect("delivery").last_status,
        Some(first)
    );
    assert_eq!(collector.counts().statuses, 1);
    assert_eq!(collector.counts().status_stale, 2);
    assert_eq!(collector.counts().invalid, 2);
}

#[test]
fn counter_regression_and_traffic_after_final_are_rejected() {
    let (_, mut collector, mut sender) = setup();
    assert_eq!(sender.send(&event()), Sent::Queued);
    collector.poll_at(Instant::now());
    raw(&mut sender, status(1, 0, true));
    collector.poll_at(Instant::now());
    assert_eq!(collector.counts().invalid, 1);
    raw(&mut sender, status(2, 1, true));
    collector.poll_at(Instant::now());
    assert_eq!(sender.inner.send(&event()), Sent::Queued);
    collector.poll_at(Instant::now());
    raw(&mut sender, status(3, 2, true));
    collector.poll_at(Instant::now());
    assert_eq!(collector.counts().invalid, 3);
    assert_eq!(collector.counts().accepted, 1);
    assert_eq!(collector.close().unknown_loss_producers, 1);
}

#[test]
fn all_loss_counters_are_monotonic_and_revision_never_wraps() {
    let initial = ProducerStatus {
        queue_loss: Loss {
            abandoned: 1,
            full: 1,
            closed: 1,
            encoding: 1,
            exhausted: 1,
        },
        transport_loss: TransportLoss {
            full: 1,
            unavailable: 1,
            invalid: 1,
        },
        ..status(1, 1, false)
    };
    let value = serde_json::to_value(initial).expect("JSON");
    for path in [
        "/events_sent",
        "/queue_loss/abandoned",
        "/queue_loss/full",
        "/queue_loss/closed",
        "/queue_loss/encoding",
        "/queue_loss/exhausted",
        "/transport_loss/full",
        "/transport_loss/unavailable",
        "/transport_loss/invalid",
    ] {
        let mut changed = value.clone();
        changed["revision"] = json!(2);
        *changed.pointer_mut(path).expect("counter") = json!(0);
        assert!(!monotonic(
            initial,
            serde_json::from_value(changed).expect("typed status")
        ));
    }
    let (_, _, mut sender) = setup();
    sender.next_revision = u64::MAX;
    assert_eq!(sender.report(Loss::default(), true), Report::Exhausted);
    assert_eq!(sender.next_revision, u64::MAX);
    sender.events_sent = u64::MAX;
    assert_eq!(sender.send(&event()), Sent::Invalid);
}

#[test]
fn local_close_accounts_for_abandoned_queue_without_waiting_for_a_payload() {
    use std::sync::{Arc, mpsc};
    let (emitter, collector, _) = setup();
    let emitter = Arc::new(emitter);
    let prepare = || {
        (
            Correlation::default(),
            Fact::Client {
                outcome: ClientOutcome::Connected,
            },
        )
    };
    for _ in 0..3 {
        assert_eq!(emitter.emit(prepare), Emitted::Queued);
    }
    let (entered, entered_rx) = mpsc::sync_channel(1);
    let (release, release_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn({
        let emitter = emitter.clone();
        move || {
            emitter.emit(|| {
                entered.send(()).expect("entered");
                let _ = release_rx.recv_timeout(Duration::from_secs(3));
                prepare()
            })
        }
    });
    entered_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("payload barrier");
    let now = Instant::now();
    let summary = collector.close();
    let elapsed = now.elapsed();
    release.send(()).expect("release");
    assert_eq!(worker.join().expect("payload thread"), Emitted::Closed);
    assert!(elapsed < Duration::from_secs(1));
    assert_eq!(summary.local_loss.abandoned, 3);
    assert!(summary.local_unconfirmed);
    assert!(summary.incomplete());
    assert_eq!(
        emitter
            .loss
            .active
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[test]
fn summary_marks_eviction_and_unknown_usage_as_incomplete() {
    for counts in [
        Counts {
            dedup_evicted: 1,
            ..Counts::default()
        },
        Counts {
            usage_evicted: 1,
            ..Counts::default()
        },
        Counts {
            usage_unattributed: 1,
            ..Counts::default()
        },
        Counts {
            refused_producers: 1,
            ..Counts::default()
        },
    ] {
        let summary = super::super::Summary {
            counts,
            closed: true,
            ..Default::default()
        };
        assert!(summary.incomplete());
    }
}
