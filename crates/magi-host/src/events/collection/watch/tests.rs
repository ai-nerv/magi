#![allow(clippy::expect_used)]

use super::*;

#[test]
fn rejected_thread_closure_fails_watch_before_owner_cleanup() {
    let watch = Watch::default();
    let stop = Arc::new(AtomicBool::new(false));
    let guard = watch.pending(stop.clone());
    let launch = move || {
        let _guard = guard.enter();
    };
    assert!(!watch.failed() && !stop.load(Ordering::Acquire));
    drop(launch);
    assert!(watch.failed() && stop.load(Ordering::Acquire));
}

#[test]
fn normal_entered_return_does_not_publish_failure() {
    let watch = Watch::default();
    let stop = Arc::new(AtomicBool::new(false));
    let guard = watch.pending(stop.clone());
    let launch = move || {
        let _guard = guard.enter();
    };
    launch();
    assert!(!watch.failed() && !stop.load(Ordering::Acquire));
}

#[test]
fn failure_publication_happens_after_admission_is_fenced() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    for _ in 0..128 {
        let watch = Watch::default();
        let stop = Arc::new(AtomicBool::new(false));
        let guard = watch.pending(stop.clone());
        let worker = std::thread::spawn(move || drop(guard));
        while !watch.failed() {
            assert!(
                std::time::Instant::now() < deadline,
                "failure publication deadline"
            );
            std::thread::yield_now();
        }
        assert!(stop.load(Ordering::Acquire));
        worker.join().expect("guard joined");
    }
}
