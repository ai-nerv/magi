#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::super::super::{ClientOutcome, Correlation, Emitted, Emitter, Fact, Loss};
use super::super::{Counts, Report, Sent};
use super::*;
use serde_json::json;

mod native_child;
mod observer;

fn id(n: u64) -> Id {
    Id::try_from(format!("{n:032x}")).expect("identity")
}

fn setup() -> (Emitter, Collector) {
    let (emitter, receiver) = Emitter::new(id(1), [1, 2, 3]);
    (emitter, Collector::new(receiver))
}

fn event(producer: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema":1, "program":"magi", "program_version":[1,2,3],
        "producer":format!("{producer:032x}"), "sequence":1, "correlation":{},
        "fact":{"kind":"client", "outcome":"connected"}
    }))
    .expect("event")
}

fn prepare() -> (Correlation, Fact) {
    (
        Correlation::default(),
        Fact::Client {
            outcome: ClientOutcome::Connected,
        },
    )
}

fn wait(readout: &Readout, condition: impl Fn(Summary) -> bool) -> Summary {
    wait_within(Duration::from_secs(3), readout, condition)
}

fn wait_within(
    budget: Duration,
    readout: &Readout,
    condition: impl Fn(Summary) -> bool,
) -> Summary {
    let deadline = Instant::now() + budget;
    loop {
        let summary = readout.snapshot();
        if condition(summary) {
            return summary;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not reach barrier: {summary:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn worker_collects_local_and_remote_facts_and_joins_with_final_status() {
    let (emitter, collector) = setup();
    let worker = Worker::start(collector).expect("worker");
    let readout = worker.readout();
    let mut sender = worker
        .register(Owner::Magi, id(2))
        .expect("queued registration");
    assert_eq!(emitter.emit(prepare), Emitted::Queued);
    assert_eq!(sender.send(&event(2)), Sent::Queued);
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    wait(&readout, |summary| {
        summary.retired.final_confirmed == 1 && summary.counts.accepted == 2
    });
    let start = Instant::now();
    let summary = worker.shutdown().expect("joined shutdown");
    assert!(start.elapsed() < SHUTDOWN_BUDGET);
    assert!(!summary.incomplete());
    assert_eq!(summary, readout.snapshot());
    assert_eq!(
        emitter.emit(|| panic!("closed receiver cannot prepare a payload")),
        Emitted::Closed
    );
}

#[test]
fn duplicate_registration_drops_only_the_rejected_channel_and_returns_its_lease() {
    let (_, collector) = setup();
    let worker = Worker::start(collector).expect("worker");
    let readout = worker.readout();
    let mut first = worker.register(Owner::Magi, id(2)).expect("first");
    let mut duplicate = worker
        .register(Owner::Casper, id(2))
        .expect("queued duplicate");
    wait(&readout, |summary| summary.counts.refused_producers == 1);
    assert_eq!(duplicate.send(&event(2)), Sent::Unavailable);
    assert_eq!(readout.slots.load(Ordering::Acquire), 2);
    assert_eq!(first.send(&event(2)), Sent::Queued);
    assert_eq!(
        first.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    wait(&readout, |summary| summary.retired.final_confirmed == 1);
    assert!(worker.shutdown().expect("shutdown").incomplete());
}

#[test]
fn paused_worker_bounds_all_pending_and_active_channels_without_blocking_registration() {
    let (_, collector) = setup();
    let (release, released) = mpsc::sync_channel(1);
    let worker = Worker::spawn(collector, move |collector, receiver, stop, published| {
        let _ = released.recv_timeout(Duration::from_secs(3));
        run(collector, receiver, stop, published)
    })
    .expect("paused worker");
    let readout = worker.readout();
    let started = Instant::now();
    let mut senders = Vec::new();
    for n in 2..=MAX_PRODUCERS as u64 {
        senders.push(
            worker
                .register(Owner::Magi, id(n))
                .expect("bounded registration"),
        );
    }
    let refused = worker.register(Owner::Magi, id(100));
    let summary = readout.snapshot();
    let elapsed = started.elapsed();
    release.send(()).expect("release worker");
    assert!(matches!(refused, Err(RegisterError::Capacity)));
    assert!(elapsed < SHUTDOWN_BUDGET);
    assert_eq!(summary.remote_producers, MAX_PRODUCERS - 1);
    assert_eq!(summary.unknown_loss_producers, MAX_PRODUCERS - 1);
    assert_eq!(summary.counts.refused_producers, 1);
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.remote_producers, MAX_PRODUCERS - 1);
    assert!(summary.incomplete());
    for sender in &mut senders {
        assert_eq!(sender.send(&event(2)), Sent::Unavailable);
    }
}

#[test]
fn concurrent_registration_cannot_exceed_the_process_channel_bound() {
    let (_, collector) = setup();
    let worker = Worker::start(collector).expect("worker");
    let tasks: Vec<_> = (2..66)
        .map(|n| {
            let registrar = worker.registrar();
            thread::spawn(move || registrar.register(Owner::Magi, id(n)))
        })
        .collect();
    let results: Vec<_> = tasks
        .into_iter()
        .map(|task| task.join().expect("registrar"))
        .collect();
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        MAX_PRODUCERS - 1
    );
    assert_eq!(
        worker.readout().slots.load(Ordering::Acquire),
        MAX_PRODUCERS
    );
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.remote_producers, MAX_PRODUCERS - 1);
    assert_eq!(
        summary.counts.refused_producers,
        64 - (MAX_PRODUCERS - 1) as u64
    );
}

#[test]
fn owner_exit_before_admission_reclaims_pending_slots_without_losing_unknown_status() {
    let (_, collector) = setup();
    let (release, released) = mpsc::sync_channel(1);
    let worker = Worker::spawn(collector, move |collector, receiver, stop, published| {
        let _ = released.recv_timeout(Duration::from_secs(3));
        run(collector, receiver, stop, published)
    })
    .expect("paused worker");
    for _ in 1..MAX_PRODUCERS {
        let opened = worker.open(Owner::Magi).expect("pending producer");
        drop(opened.lifetime);
        drop(opened.sender);
    }
    let readout = worker.readout();
    let before = readout.snapshot();
    assert_eq!(before.remote_producers, MAX_PRODUCERS - 1);
    assert_eq!(before.unknown_loss_producers, MAX_PRODUCERS - 1);
    assert_eq!(readout.slots.load(Ordering::Acquire), MAX_PRODUCERS);
    release.send(()).expect("release worker");
    wait(&readout, |summary| {
        summary.retired.producers == (MAX_PRODUCERS - 1) as u64
    });
    assert_eq!(readout.slots.load(Ordering::Acquire), 1);
    let replacement = worker.open(Owner::Magi).expect("reusable capacity");
    drop(replacement);
    wait(&readout, |summary| {
        summary.retired.producers == MAX_PRODUCERS as u64
    });
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.remote_producers, 0);
    assert_eq!(summary.retired.unknown_loss_producers, MAX_PRODUCERS as u64);
    assert!(summary.incomplete());
}

#[test]
fn dropping_worker_wakes_and_joins_its_actual_native_thread() {
    fn instance(path: &std::path::Path) -> Option<String> {
        let stat = std::fs::read_to_string(path.join("stat")).ok()?;
        stat.rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(19)
            .map(str::to_owned)
    }
    let (_, collector) = setup();
    let (reported, identity) = mpsc::sync_channel(1);
    let worker = Worker::spawn(collector, move |collector, receiver, stop, published| {
        reported
            .send(std::fs::read_link("/proc/thread-self").expect("native thread identity"))
            .expect("thread identity");
        run(collector, receiver, stop, published)
    })
    .expect("worker");
    let path = std::path::Path::new("/proc").join(
        identity
            .recv_timeout(Duration::from_secs(3))
            .expect("identity deadline"),
    );
    let readout = worker.readout();
    let registrar = worker.registrar();
    let identity = instance(&path).expect("live thread instance");
    let started = Instant::now();
    drop(worker);
    while instance(&path).as_ref() == Some(&identity) {
        assert!(
            started.elapsed() < SHUTDOWN_BUDGET,
            "joined native thread instance remained past shutdown deadline"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.elapsed() < SHUTDOWN_BUDGET);
    assert!(readout.snapshot().closed);
    assert!(matches!(
        registrar.open(Owner::Magi),
        Err(RegisterError::Closed)
    ));
    assert_eq!(readout.snapshot().counts.refused_producers, 1);
}

#[test]
fn busy_admission_never_waits_and_returns_every_reserved_slot() {
    let (_, collector) = setup();
    let worker = Worker::start(collector).expect("worker");
    let registrar = worker.registrar();
    let readout = registrar.readout();
    let admission = registrar.registrations.write().expect("admission fence");
    let start = Instant::now();
    for _ in 0..MAX_PRODUCERS {
        let clone = registrar.clone();
        assert!(matches!(clone.open(Owner::Magi), Err(RegisterError::Busy)));
        assert_eq!(readout.slots.load(Ordering::Acquire), 1);
    }
    assert!(start.elapsed() < SHUTDOWN_BUDGET);
    assert_eq!(
        readout.snapshot().counts.refused_producers,
        MAX_PRODUCERS as u64
    );
    drop(admission);
    let lineage = Lineage::root(id(20), id(21));
    let mut opened = registrar
        .open_bound(Owner::Magi, lineage)
        .expect("available");
    assert_eq!(opened.sender.lineage(), lineage);
    assert_ne!(opened.sender.identity(), id(1));
    drop(registrar);
    assert_eq!(
        opened.sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    drop(opened);
    wait(&readout, |summary| summary.retired.final_confirmed == 1);
    assert_eq!(readout.slots.load(Ordering::Acquire), 1);
    assert_eq!(worker.shutdown().expect("shutdown").retired.producers, 1);
}

#[test]
fn shutdown_accounts_for_every_successful_racing_registration() {
    let (_, collector) = setup();
    let worker = Worker::start(collector).expect("worker");
    let readout = worker.readout();
    let first = worker.open(Owner::Magi).expect("first producer");
    let barrier = Arc::new(std::sync::Barrier::new(65));
    let tasks: Vec<_> = (0..64)
        .map(|_| {
            let registrar = worker.registrar();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                registrar.open(Owner::Magi)
            })
        })
        .collect();
    barrier.wait();
    let closed = worker.shutdown().expect("shutdown");
    let opened: Vec<_> = tasks
        .into_iter()
        .filter_map(|task| match task.join().expect("registrar") {
            Ok(opened) => Some(opened),
            Err(RegisterError::Closed | RegisterError::Capacity | RegisterError::Busy) => None,
            Err(error) => panic!("unexpected refusal: {error:?}"),
        })
        .collect();
    let summary = readout.snapshot();
    assert!(summary.closed && closed.closed);
    assert_eq!(summary.remote_producers, 1 + opened.len());
    assert_eq!(summary.remote_producers, closed.remote_producers);
    assert_eq!(summary.unknown_loss_producers, summary.remote_producers);
    assert!(summary.remote_producers < MAX_PRODUCERS);
    assert_eq!(summary.counts.refused_producers, 64 - opened.len() as u64);
    assert_eq!(readout.slots.load(Ordering::Acquire), 1);
    drop(first);
    drop(opened);
}

#[test]
fn flooded_senders_keep_doing_useful_work_when_collector_shuts_down() {
    let (_, collector) = setup();
    let worker = Worker::start(collector).expect("worker");
    let mut sender = worker.register(Owner::Magi, id(2)).expect("sender");
    let (entered, barrier) = mpsc::sync_channel(1);
    let thread = thread::spawn(move || {
        let bytes = event(2);
        let mut useful = 0;
        for n in 0..10000 {
            if n == 100 {
                entered.send(()).expect("work barrier");
            }
            useful += 1;
            assert!(matches!(
                sender.send(&bytes),
                Sent::Queued | Sent::Full | Sent::Unavailable
            ));
        }
        useful
    });
    barrier
        .recv_timeout(Duration::from_secs(3))
        .expect("useful work");
    let started = Instant::now();
    let summary = worker.shutdown().expect("shutdown");
    let elapsed = started.elapsed();
    assert_eq!(thread.join().expect("sender joined"), 10000);
    assert!(elapsed < SHUTDOWN_BUDGET);
    assert_eq!(summary.unknown_loss_producers, 1);
    assert!(summary.incomplete());
}

#[test]
fn worker_panic_and_shutdown_overrun_are_visible_not_detached() {
    let (_, collector) = setup();
    let worker =
        Worker::spawn(collector, |_, _, _, _| panic!("synthetic worker failure")).expect("worker");
    let readout = worker.readout();
    wait(&readout, |summary| summary.worker_failed);
    assert_eq!(worker.shutdown(), Err(ShutdownError::Panicked));
    let summary = readout.snapshot();
    assert!(summary.closed && summary.worker_failed && summary.incomplete());
    let (_, collector) = setup();
    let mut worker = Worker::start(collector).expect("worker");
    let readout = worker.readout();
    assert_eq!(
        worker.finish_started(Instant::now() - Duration::from_secs(2)),
        Err(ShutdownError::BudgetExceeded)
    );
    assert!(worker.task.is_none());
    assert!(readout.snapshot().shutdown_overrun);
    assert!(readout.snapshot().incomplete());
}

#[test]
fn atomic_publication_preserves_all_fixed_fields_after_close() {
    let published = Published::default();
    let summary = Summary {
        counts: Counts {
            accepted: 1,
            duplicate: 2,
            invalid: 3,
            unavailable: 4,
            refused_producers: 5,
            dedup_evicted: 6,
            usage_evicted: 7,
            usage_unattributed: 8,
            usage_stale: 9,
            usage_conflicts: 10,
            statuses: 11,
            status_stale: 12,
        },
        local_loss: Loss {
            abandoned: 13,
            full: 14,
            closed: 15,
            encoding: 16,
            exhausted: 17,
        },
        remote_producers: 18,
        final_confirmed: 19,
        unknown_loss_producers: 20,
        reported_loss_producers: 21,
        retired: super::super::Retired {
            producers: 22,
            final_confirmed: 23,
            unknown_loss_producers: 24,
            reported_loss_producers: 25,
            queue_loss: Loss {
                abandoned: 26,
                full: 27,
                closed: 28,
                encoding: 29,
                exhausted: 30,
            },
            transport_loss: super::super::TransportLoss {
                full: 31,
                unavailable: 32,
                invalid: 33,
            },
        },
        local_unconfirmed: true,
        worker_failed: true,
        shutdown_overrun: true,
        closed: true,
    };
    published.store(summary);
    assert_eq!(published.load(), summary);
}

#[test]
fn panicking_payload_releases_its_active_emission_guard() {
    let (emitter, collector) = setup();
    assert!(std::panic::catch_unwind(|| emitter.emit(|| panic!("payload"))).is_err());
    let summary = collector.close();
    assert!(!summary.local_unconfirmed);
    assert_eq!(summary.local_loss.encoding, 1);
    assert!(summary.incomplete());
    assert_eq!(emitter.loss.active.load(Ordering::SeqCst), 0);
}

#[test]
fn active_emission_exhaustion_never_wraps_or_prepares_a_payload() {
    let (emitter, collector) = setup();
    emitter.loss.active.store(u64::MAX, Ordering::SeqCst);
    assert_eq!(
        emitter.emit(|| panic!("exhausted active counter")),
        Emitted::Exhausted
    );
    assert_eq!(emitter.loss.active.load(Ordering::SeqCst), u64::MAX);
    let summary = collector.close();
    assert!(summary.local_unconfirmed && summary.incomplete());
    assert_eq!(summary.local_loss.exhausted, 1);
}

#[test]
fn live_readout_keeps_mixed_publication_counts_within_registered_capacity() {
    let readout = Readout {
        published: Arc::new(Published::default()),
        refused: Arc::new(AtomicU64::new(0)),
        slots: Arc::new(AtomicUsize::new(MAX_PRODUCERS)),
        producers: Arc::new(AtomicUsize::new(MAX_PRODUCERS - 1)),
    };
    readout.published.store(Summary {
        remote_producers: 0,
        unknown_loss_producers: MAX_PRODUCERS - 1,
        reported_loss_producers: MAX_PRODUCERS - 1,
        ..Summary::default()
    });
    let summary = readout.snapshot();
    assert_eq!(summary.remote_producers, MAX_PRODUCERS - 1);
    assert_eq!(summary.unknown_loss_producers, MAX_PRODUCERS - 1);
    assert_eq!(
        summary.final_confirmed + summary.unknown_loss_producers,
        summary.remote_producers
    );
    assert!(summary.incomplete());
}
