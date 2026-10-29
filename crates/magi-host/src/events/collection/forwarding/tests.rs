#![allow(clippy::expect_used)]

use super::super::{Collector, Opened, Sent, bootstrap::tests::native::isolate};
use super::*;
use crate::events::{self, ClientOutcome, Correlation, Emitted, Emitter, Fact, Id, QUEUED_EVENTS};
use std::sync::mpsc;

fn setup() -> (Collector, Emitter, Receiver, Opened) {
    let (_, receiver) = Emitter::new(Id::generate().expect("root identity"), [1, 2, 3]);
    let mut collector = Collector::new(receiver);
    let opened = collector.open(Owner::Magi).expect("producer");
    let (emitter, receiver) = Emitter::new(opened.sender.identity(), [1, 2, 3]);
    (collector, emitter, receiver, opened)
}

fn prepared() -> (Correlation, Fact) {
    (
        Correlation::default(),
        Fact::Client {
            outcome: ClientOutcome::Connected,
        },
    )
}

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "forwarder barrier deadline");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn forwarded_events_preserve_binding_and_end_with_reconciled_delivery() {
    let (mut collector, emitter, receiver, Opened { sender, lifetime }) = setup();
    let forwarder = Forwarder::spawn(receiver, sender, run).expect("forwarder");
    assert_eq!(emitter.emit(prepared), Emitted::Queued);
    wait(|| {
        collector.poll();
        collector.counts().accepted == 1
    });
    let started = Instant::now();
    let outcome = forwarder.shutdown().expect("joined forwarder");
    assert!(started.elapsed() < SHUTDOWN_BUDGET);
    assert!(!outcome.incomplete());
    collector.poll();
    drop(lifetime);
    let summary = collector.close();
    assert_eq!(summary.retired.final_confirmed, 1);
    assert!(!summary.incomplete());
    assert_eq!(
        emitter.emit(|| panic!("closed forwarding queue")),
        Emitted::Closed
    );
}

#[test]
fn shutdown_abandons_queued_events_with_explicit_loss_instead_of_flushing_forever() {
    let (mut collector, emitter, receiver, Opened { sender, lifetime }) = setup();
    for _ in 0..QUEUED_EVENTS {
        assert_eq!(emitter.emit(prepared), Emitted::Queued);
    }
    assert_eq!(emitter.emit(prepared), Emitted::Full);
    let forwarder = Forwarder::spawn(receiver, sender, |receiver, sender, _| (receiver, sender))
        .expect("worker");
    let outcome = forwarder.shutdown().expect("shutdown");
    assert_eq!(outcome.queue_loss.abandoned, QUEUED_EVENTS as u64);
    assert_eq!(outcome.queue_loss.full, 1);
    assert!(outcome.incomplete());
    collector.poll();
    drop(lifetime);
    let summary = collector.close();
    assert_eq!(summary.retired.final_confirmed, 1);
    assert_eq!(summary.retired.queue_loss, outcome.queue_loss);
    assert!(summary.incomplete());
}

#[test]
fn full_transport_drops_without_delaying_useful_producer_work() {
    let (
        _collector,
        emitter,
        receiver,
        Opened {
            mut sender,
            lifetime,
        },
    ) = setup();
    let mut full = false;
    for _ in 0..10000 {
        if sender.send(&[b'x'; events::MAX_EVENT_BYTES]) == Sent::Full {
            full = true;
            break;
        }
    }
    assert!(full);
    let forwarder = Forwarder::spawn(receiver, sender, run).expect("forwarder");
    let started = Instant::now();
    let mut useful = 0;
    for _ in 0..1000 {
        useful += 1;
        assert!(matches!(
            emitter.emit(prepared),
            Emitted::Queued | Emitted::Full
        ));
    }
    assert_eq!(useful, 1000);
    assert!(started.elapsed() < SHUTDOWN_BUDGET);
    wait(|| emitter.loss.dequeued.load(Ordering::Relaxed) != 0);
    let outcome = forwarder.shutdown().expect("shutdown");
    assert!(outcome.transport_loss.full >= 2);
    assert_eq!(outcome.final_report, Report::Sent(Sent::Full));
    assert!(outcome.incomplete());
    drop(lifetime);
}

#[test]
fn closed_collector_does_not_reconnect_or_block_native_emission() {
    let (collector, emitter, receiver, Opened { sender, lifetime }) = setup();
    collector.close();
    let forwarder = Forwarder::spawn(receiver, sender, run).expect("forwarder");
    assert_eq!(emitter.emit(prepared), Emitted::Queued);
    wait(|| emitter.loss.dequeued.load(Ordering::Relaxed) == 1);
    let outcome = forwarder.shutdown().expect("shutdown");
    assert_eq!(outcome.transport_loss.unavailable, 2);
    assert_eq!(outcome.final_report, Report::Sent(Sent::Unavailable));
    assert!(outcome.incomplete());
    drop(lifetime);
}

#[test]
fn in_flight_payload_is_unknown_at_shutdown_and_never_waited_on() {
    let (mut collector, emitter, receiver, Opened { sender, lifetime }) = setup();
    let emitter = Arc::new(emitter);
    let forwarder = Forwarder::spawn(receiver, sender, run).expect("forwarder");
    let (entered, entering) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let producer = thread::spawn({
        let emitter = emitter.clone();
        move || {
            emitter.emit(|| {
                entered.send(()).expect("entered");
                let _ = released.recv_timeout(Duration::from_secs(3));
                prepared()
            })
        }
    });
    entering
        .recv_timeout(Duration::from_secs(3))
        .expect("payload barrier");
    let started = Instant::now();
    let outcome = forwarder.shutdown();
    let elapsed = started.elapsed();
    release.send(()).expect("release");
    assert_eq!(producer.join().expect("producer"), Emitted::Closed);
    let outcome = outcome.expect("forwarder shutdown");
    assert!(elapsed < SHUTDOWN_BUDGET);
    assert!(outcome.local_unconfirmed && outcome.incomplete());
    drop(lifetime);
    collector.poll();
    assert_eq!(collector.close().retired.unknown_loss_producers, 1);
}

#[test]
fn final_report_rate_limit_never_turns_into_a_shutdown_wait() {
    let (mut collector, _, receiver, Opened { sender, lifetime }) = setup();
    let forwarder = Forwarder::spawn(receiver, sender, |receiver, mut sender, _| {
        assert_eq!(
            sender.report(Loss::default(), false),
            Report::Sent(Sent::Queued)
        );
        (receiver, sender)
    })
    .expect("worker");
    let started = Instant::now();
    let outcome = forwarder.shutdown().expect("shutdown");
    assert!(started.elapsed() < SHUTDOWN_BUDGET);
    assert_eq!(outcome.final_report, Report::RateLimited);
    assert!(outcome.incomplete());
    drop(lifetime);
    collector.poll();
    assert_eq!(collector.close().retired.unknown_loss_producers, 1);
}

#[test]
fn forwarder_panic_and_overrun_are_joined_failures() {
    let (collector, emitter, receiver, Opened { sender, lifetime }) = setup();
    let forwarder = Forwarder::spawn(receiver, sender, |_, _, _| {
        panic!("synthetic forwarder failure")
    })
    .expect("worker");
    assert_eq!(forwarder.shutdown(), Err(ShutdownError::Panicked));
    assert_eq!(emitter.emit(prepared), Emitted::Closed);
    drop(lifetime);
    assert!(collector.close().incomplete());
    let (mut collector, _, receiver, Opened { sender, lifetime }) = setup();
    let mut forwarder = Forwarder::spawn(receiver, sender, run).expect("worker");
    assert_eq!(
        forwarder.finish(Instant::now() - Duration::from_secs(2)),
        Err(ShutdownError::BudgetExceeded)
    );
    assert!(forwarder.task.is_none());
    drop(lifetime);
    collector.poll();
    assert_eq!(collector.close().retired.unknown_loss_producers, 1);
}

#[test]
fn explicit_forwarding_activation_binds_the_process_once() {
    if isolate(
        "events::collection::forwarding::tests::explicit_forwarding_activation_binds_the_process_once",
    ) {
        return;
    }
    assert_eq!(
        events::emit(|| panic!("default disabled")),
        Emitted::Disabled
    );
    let (mut collector, _, _, Opened { sender, lifetime }) = setup();
    let forwarder = Forwarder::activate(sender).expect("explicit native activation");
    assert!(matches!(
        events::enable_local(),
        Err(EnableError::AlreadyEnabled)
    ));
    assert_eq!(events::emit(prepared), Emitted::Queued);
    wait(|| {
        collector.poll();
        collector.counts().accepted == 1
    });
    let outcome = forwarder.shutdown().expect("shutdown");
    assert!(!outcome.incomplete());
    collector.poll();
    drop(lifetime);
    assert_eq!(collector.close().retired.final_confirmed, 1);
    assert_eq!(events::emit(prepared), Emitted::Closed);
}

#[test]
fn wrong_owner_and_used_sender_cannot_activate_a_process_queue() {
    if isolate(
        "events::collection::forwarding::tests::wrong_owner_and_used_sender_cannot_activate_a_process_queue",
    ) {
        return;
    }
    let (mut collector, _, _, opened) = setup();
    let wrong = collector.open(Owner::Casper).expect("other owner");
    assert!(matches!(
        Forwarder::activate(wrong.sender),
        Err(ForwardError::Owner)
    ));
    assert_eq!(events::emit(prepared), Emitted::Disabled);
    let Opened {
        mut sender,
        lifetime,
    } = opened;
    assert_eq!(sender.send(b""), Sent::Invalid);
    assert!(matches!(
        Forwarder::activate(sender),
        Err(ForwardError::UsedSender)
    ));
    assert_eq!(events::emit(prepared), Emitted::Disabled);
    drop((lifetime, wrong.lifetime));
}
