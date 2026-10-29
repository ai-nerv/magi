#![allow(clippy::expect_used)]

use super::*;
use crate::events::{
    Emitter,
    collection::{Collector, Worker as CollectorWorker, enrollment::Client},
};
use std::time::Instant;

#[test]
fn panicking_tree_worker_fences_admission_closes_pending_hosts_and_is_joined() {
    let (_, receiver) = Emitter::new(Id::generate().expect("producer"), [1, 0, 0]);
    let collector = CollectorWorker::start(Collector::new(receiver)).expect("collector");
    let watch = Watch::default();
    let (started, ready) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let tree = Worker::spawn(collector.registrar(), watch.clone(), move || {
        started
            .send(std::fs::read_link("/proc/thread-self").expect("native thread identity"))
            .expect("native thread");
        released
            .recv_timeout(Duration::from_secs(3))
            .expect("release");
        panic!("controlled tree failure");
    })
    .expect("tree");
    let tid = ready
        .recv_timeout(Duration::from_secs(3))
        .expect("thread entered");
    let task = std::path::Path::new("/proc").join(tid);
    assert!(std::path::Path::new(&task).exists());
    let admission = tree.admission();
    let parent = Lineage::root(Id::generate().expect("run"), Id::generate().expect("agent"));
    let (sender, control, _) = admission
        .prepare(parent)
        .expect("queued branch")
        .into_parts();
    let client = Client::adopt(control, sender.lineage()).expect("client");
    assert_eq!(admission.slots.load(Ordering::Acquire), 1);
    release.send(()).expect("panic release");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !watch.failed() {
        assert!(Instant::now() < deadline, "failure publication deadline");
        thread::sleep(Duration::from_millis(2));
    }
    assert!(admission.prepare(parent).is_none());
    drop(tree);
    assert_eq!(admission.slots.load(Ordering::Acquire), 0);
    assert!(!std::path::Path::new(&task).exists());
    assert!(client.poll().is_err());
    assert!(client.is_closed());
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
            "pending host retirement deadline"
        );
        thread::sleep(Duration::from_millis(2));
    }
    let summary = collector.shutdown().expect("collector shutdown");
    assert_eq!(summary.retired.unknown_loss_producers, 1);
    assert!(summary.incomplete());
}
