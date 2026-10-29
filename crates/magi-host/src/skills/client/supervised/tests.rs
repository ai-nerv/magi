use super::*;
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

mod integration;
mod saturation;

type Gate = Arc<(Mutex<bool>, Condvar)>;

struct Release(Gate);

impl Release {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), Condvar::new())))
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        *self.0.0.lock().expect("gate") = true;
        self.0.1.notify_all();
    }
}

fn wait(gate: &Gate) {
    let (held, waited) = gate
        .1
        .wait_timeout_while(
            gate.0.lock().expect("gate"),
            Duration::from_secs(5),
            |open| !*open,
        )
        .expect("gate wait");
    assert!(*held && !waited.timed_out(), "test did not release worker");
}

async fn available(slots: &Semaphore, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while slots.available_permits() != expected {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("worker cleanup released its capacity");
}

#[tokio::test(flavor = "current_thread")]
async fn timeout_returns_during_blocked_work_but_retains_capacity_until_cleanup() {
    let slots = Arc::new(Semaphore::new(1));
    let release = Release::new();
    let gate = Arc::clone(&release.0);
    let cancelled = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&cancelled);
    let (started, ready) = tokio::sync::oneshot::channel();
    let pool = Arc::clone(&slots);
    let began = Instant::now();
    let task = tokio::spawn(async move {
        bounded(
            pool,
            &Cancel::default(),
            began + Duration::from_millis(200),
            move |cancel| {
                started.send(()).expect("started");
                wait(&gate);
                seen.store(cancel.is_requested(), Ordering::SeqCst);
                Ok(7)
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(1), ready)
        .await
        .expect("responsive runtime")
        .expect("worker");
    assert_eq!(task.await.expect("joined"), Err(Error::Timeout));
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(slots.available_permits(), 0);
    let executed = Arc::new(AtomicBool::new(false));
    let marker = Arc::clone(&executed);
    assert_eq!(
        bounded(
            Arc::clone(&slots),
            &Cancel::default(),
            Instant::now() + Duration::from_millis(30),
            move |_| {
                marker.store(true, Ordering::SeqCst);
                Ok(())
            }
        )
        .await,
        Err(Error::Timeout)
    );
    assert!(!executed.load(Ordering::SeqCst));
    drop(release);
    available(&slots, 1).await;
    assert!(cancelled.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_and_dropped_callers_signal_owned_work_without_releasing_its_slot() {
    for abort in [false, true] {
        let slots = Arc::new(Semaphore::new(1));
        let pool = Arc::clone(&slots);
        let cancel = Cancel::default();
        let flag = cancel.clone();
        let release = Release::new();
        let gate = Arc::clone(&release.0);
        let stopped = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&stopped);
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            bounded(
                pool,
                &flag,
                Instant::now() + Duration::from_secs(3),
                move |cancel| {
                    started.send(()).expect("started");
                    wait(&gate);
                    seen.store(cancel.is_requested(), Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), ready)
            .await
            .expect("responsive runtime")
            .expect("worker");
        if abort {
            task.abort();
            assert!(task.await.expect_err("aborted").is_cancelled());
        } else {
            cancel.request();
            assert_eq!(
                tokio::time::timeout(Duration::from_millis(200), task)
                    .await
                    .expect("prompt cancellation")
                    .expect("joined"),
                Err(Error::Cancelled)
            );
        }
        assert_eq!(slots.available_permits(), 0);
        drop(release);
        available(&slots, 1).await;
        assert!(stopped.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn stale_waiting_work_never_runs_and_success_error_and_panic_release_capacity() {
    let slots = Arc::new(Semaphore::new(1));
    let cancel = Cancel::default();
    cancel.request();
    let until = Instant::now() + Duration::from_secs(1);
    assert_eq!(
        bounded(
            Arc::clone(&slots),
            &cancel,
            until,
            |_| -> Result<(), Error> { panic!("cancelled work ran") }
        )
        .await,
        Err(Error::Cancelled)
    );
    assert_eq!(
        bounded(
            Arc::clone(&slots),
            &Cancel::default(),
            Instant::now(),
            |_| -> Result<(), Error> { panic!("expired work ran") }
        )
        .await,
        Err(Error::Timeout)
    );
    assert_eq!(
        bounded(Arc::clone(&slots), &Cancel::default(), until, |_| Ok(7)).await,
        Ok(7)
    );
    assert_eq!(
        bounded(Arc::clone(&slots), &Cancel::default(), until, |_| Err::<
            (),
            _,
        >(
            Error::Pin
        ))
        .await,
        Err(Error::Pin)
    );
    assert_eq!(
        bounded(
            Arc::clone(&slots),
            &Cancel::default(),
            until,
            |_| -> Result<(), Error> { panic!("worker failure") }
        )
        .await,
        Err(Error::Helper)
    );
    available(&slots, 1).await;
}

#[tokio::test(flavor = "current_thread")]
async fn cleanup_still_owns_capacity_after_result_is_ready_and_deadline_returns() {
    struct Cleanup(Gate, Arc<AtomicBool>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            wait(&self.0);
            self.1.store(true, Ordering::SeqCst);
        }
    }
    let slots = Arc::new(Semaphore::new(1));
    let release = Release::new();
    let done = Arc::new(AtomicBool::new(false));
    let cleanup = Cleanup(Arc::clone(&release.0), Arc::clone(&done));
    assert_eq!(
        bounded(
            Arc::clone(&slots),
            &Cancel::default(),
            Instant::now() + Duration::from_millis(100),
            move |_| {
                let _cleanup = cleanup;
                Ok(())
            }
        )
        .await,
        Err(Error::Timeout)
    );
    assert_eq!(slots.available_permits(), 0);
    assert!(!done.load(Ordering::SeqCst));
    drop(release);
    available(&slots, 1).await;
    assert!(done.load(Ordering::SeqCst));
}
