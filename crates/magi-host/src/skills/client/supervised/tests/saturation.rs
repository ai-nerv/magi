use super::*;
use std::sync::atomic::AtomicUsize;

#[tokio::test(flavor = "current_thread")]
async fn four_stalled_workers_bound_background_work_across_repeated_timeouts() {
    let slots = Arc::new(Semaphore::new(WORKERS));
    let active = Arc::new(AtomicUsize::new(0));
    let release = Release::new();
    let mut tasks = Vec::new();
    for _ in 0..WORKERS {
        let pool = Arc::clone(&slots);
        let running = Arc::clone(&active);
        let gate = Arc::clone(&release.0);
        let (started, ready) = tokio::sync::oneshot::channel();
        tasks.push(tokio::spawn(async move {
            bounded(
                pool,
                &Cancel::default(),
                Instant::now() + Duration::from_millis(200),
                move |_| {
                    running.fetch_add(1, Ordering::SeqCst);
                    started.send(()).expect("worker started");
                    wait(&gate);
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        }));
        tokio::time::timeout(Duration::from_secs(1), ready)
            .await
            .expect("worker admitted")
            .expect("started");
    }
    for task in tasks {
        assert_eq!(task.await.expect("joined"), Err(Error::Timeout));
    }
    assert_eq!(active.load(Ordering::SeqCst), WORKERS);
    assert_eq!(slots.available_permits(), 0);
    for _ in 0..20 {
        assert_eq!(
            bounded(
                Arc::clone(&slots),
                &Cancel::default(),
                Instant::now() + Duration::from_millis(2),
                |_| -> Result<(), Error> { panic!("over-capacity work executed") }
            )
            .await,
            Err(Error::Timeout)
        );
    }
    assert_eq!(active.load(Ordering::SeqCst), WORKERS);
    drop(release);
    available(&slots, WORKERS).await;
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_capacity_waiter_is_removed_without_starting_work_after_release() {
    let slots = Arc::new(Semaphore::new(1));
    let held = slots.acquire().await.expect("held capacity");
    let pool = Arc::clone(&slots);
    let cancel = Cancel::default();
    let flag = cancel.clone();
    let task = tokio::spawn(async move {
        bounded(
            pool,
            &flag,
            Instant::now() + Duration::from_secs(3),
            |_| -> Result<(), Error> { panic!("cancelled capacity waiter ran") },
        )
        .await
    });
    tokio::task::yield_now().await;
    cancel.request();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(200), task)
            .await
            .expect("prompt queue cancellation")
            .expect("joined"),
        Err(Error::Cancelled)
    );
    drop(held);
    available(&slots, 1).await;
}
