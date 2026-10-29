#![allow(clippy::expect_used)]

use super::*;
use crate::events::{
    Emitter, Id,
    collection::{
        Collector, Worker as CollectorWorker, bootstrap::tests::native::isolate, enrollment::Host,
    },
};
use std::sync::mpsc;

#[test]
fn credit_thread_panic_closes_control_is_visible_and_never_confirms_final_delivery() {
    const CASE: &str = "events::collection::runtime::forwarding::tests::credit_thread_panic_closes_control_is_visible_and_never_confirms_final_delivery";
    if isolate(CASE) {
        return;
    }
    let (_, receiver) = Emitter::new(Id::generate().expect("producer"), [1, 0, 0]);
    let collector = CollectorWorker::start(Collector::new(receiver)).expect("collector");
    let run = Id::generate().expect("run");
    let parent = Lineage::root(run, Id::generate().expect("parent"));
    let lineage = Lineage::root(run, Id::generate().expect("child"));
    let (mut host, sender, control) = Host::pair(collector.registrar(), lineage).expect("host");
    let branch = Branch::new(sender, control, parent).expect("branch");
    let watch = Watch::default();
    let (started, ready) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let worker = Worker::spawn(branch, watch.clone(), move || {
        started
            .send(std::fs::read_link("/proc/thread-self").expect("native thread identity"))
            .expect("native thread");
        released
            .recv_timeout(Duration::from_secs(3))
            .expect("release");
        panic!("controlled credit failure");
    })
    .expect("forwarding worker");
    let tid = ready
        .recv_timeout(Duration::from_secs(3))
        .expect("thread entered");
    let task = std::path::Path::new("/proc").join(tid);
    assert!(std::path::Path::new(&task).exists());
    let client = worker.client.clone();
    release.send(()).expect("panic release");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !watch.failed() || !client.is_closed() {
        assert!(Instant::now() < deadline, "credit failure publication");
        thread::sleep(Duration::from_millis(2));
    }
    worker
        .shutdown_started(Instant::now())
        .expect("worker cleanup");
    assert!(!std::path::Path::new(&task).exists());
    assert!(!host.poll());
    let deadline = Instant::now() + Duration::from_secs(3);
    while collector
        .readout()
        .snapshot()
        .retired
        .unknown_loss_producers
        != 1
    {
        assert!(
            Instant::now() < deadline,
            "failed child retirement deadline"
        );
        thread::sleep(Duration::from_millis(2));
    }
    let summary = collector.shutdown().expect("collector shutdown");
    assert_eq!(summary.retired.final_confirmed, 0);
    assert_eq!(summary.retired.unknown_loss_producers, 1);
    assert!(summary.incomplete());
}

fn independent_branch() -> (
    CollectorWorker,
    crate::events::collection::Lifetime,
    Branch,
    rustix::fd::OwnedFd,
) {
    use rustix::net::{self, AddressFamily, SocketFlags, SocketType};
    let (_, receiver) = Emitter::new(Id::generate().expect("producer"), [1, 0, 0]);
    let collector = CollectorWorker::start(Collector::new(receiver)).expect("collector");
    let run = Id::generate().expect("run");
    let lineage = Lineage::root(run, Id::generate().expect("child"));
    let parent = Lineage::root(run, Id::generate().expect("parent"));
    let opened = collector
        .open_bound(crate::events::collection::Owner::Magi, lineage)
        .expect("producer");
    let (root, peer) = net::socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .expect("control");
    crate::events::collection::transport::configure(&root).expect("root limits");
    crate::events::collection::transport::configure(&peer).expect("child limits");
    let branch = Branch::new(opened.sender, peer, parent).expect("branch");
    (collector, opened.lifetime, branch, root)
}

fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready() {
        assert!(Instant::now() < deadline, "collection lifecycle deadline");
        thread::sleep(Duration::from_millis(2));
    }
}

fn control_failure(closed: bool) {
    let (collector, lifetime, branch, control) = independent_branch();
    let watch = Watch::default();
    let worker = Worker::start(branch, watch.clone()).expect("forwarding scope");
    let client = worker.client.clone();
    wait(|| collector.readout().snapshot().counts.accepted == 1);
    if closed {
        rustix::net::shutdown(&control, rustix::net::Shutdown::ReadWrite).expect("control EOF");
    } else {
        rustix::net::send(
            &control,
            b"invalid-control-frame",
            rustix::net::SendFlags::DONTWAIT | rustix::net::SendFlags::NOSIGNAL,
        )
        .expect("malformed control");
    }
    wait(|| client.is_closed() && watch.failed());
    worker
        .shutdown_started(Instant::now())
        .expect("joined cleanup");
    drop(lifetime);
    wait(|| collector.readout().snapshot().retired.producers == 1);
    let summary = collector.shutdown().expect("collector shutdown");
    assert_eq!(summary.retired.final_confirmed, 0);
    assert_eq!(summary.retired.unknown_loss_producers, 1);
    assert_eq!(summary.counts.invalid, 0);
    assert!(summary.incomplete());
}

#[test]
fn control_eof_cannot_confirm_delivery_on_a_still_live_event_channel() {
    const CASE: &str = "events::collection::runtime::forwarding::tests::control_eof_cannot_confirm_delivery_on_a_still_live_event_channel";
    if isolate(CASE) {
        return;
    }
    control_failure(true);
}

#[test]
fn malformed_control_cannot_confirm_delivery_on_a_still_live_event_channel() {
    const CASE: &str = "events::collection::runtime::forwarding::tests::malformed_control_cannot_confirm_delivery_on_a_still_live_event_channel";
    if isolate(CASE) {
        return;
    }
    control_failure(false);
}

#[test]
fn slow_credit_join_consumes_the_original_shutdown_budget() {
    const CASE: &str = "events::collection::runtime::forwarding::tests::slow_credit_join_consumes_the_original_shutdown_budget";
    if isolate(CASE) {
        return;
    }
    let (collector, lifetime, branch, _control) = independent_branch();
    let watch = Watch::default();
    let (ready, entered) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let worker = Worker::spawn(branch, watch.clone(), move || {
        ready.send(()).expect("credit thread entered");
        released
            .recv_timeout(Duration::from_secs(3))
            .expect("credit release");
    })
    .expect("worker");
    entered
        .recv_timeout(Duration::from_secs(3))
        .expect("credit barrier");
    wait(|| collector.readout().snapshot().counts.accepted == 1);
    let started = Instant::now();
    let releaser = thread::spawn(move || {
        thread::sleep(crate::events::collection::SHUTDOWN_BUDGET + Duration::from_millis(50));
        release.send(()).expect("release credit thread");
    });
    assert!(matches!(
        worker.shutdown_started(started),
        Err(ShutdownError::BudgetExceeded)
    ));
    releaser.join().expect("release joined");
    assert!(!watch.failed());
    drop(lifetime);
    wait(|| collector.readout().snapshot().retired.producers == 1);
    let summary = collector.shutdown().expect("collector shutdown");
    assert_eq!(summary.retired.final_confirmed, 0);
    assert_eq!(summary.retired.unknown_loss_producers, 1);
}
