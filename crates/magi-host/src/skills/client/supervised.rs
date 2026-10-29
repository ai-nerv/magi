use super::{Cancel, Error};
use std::sync::{Arc, LazyLock};
use std::time::Instant;
use tokio::sync::Semaphore;

const WORKERS: usize = 4;
static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(WORKERS)));

struct Running(Cancel);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.request();
    }
}

pub(super) fn check(cancel: &Cancel, deadline: Instant) -> Result<(), Error> {
    if cancel.is_requested() {
        Err(Error::Cancelled)
    } else if Instant::now() >= deadline {
        Err(Error::Timeout)
    } else {
        Ok(())
    }
}

pub(super) async fn run<T: Send + 'static>(
    cancel: &Cancel,
    deadline: Instant,
    work: impl FnOnce(Cancel) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    bounded(Arc::clone(&SLOTS), cancel, deadline, work).await
}

async fn bounded<T: Send + 'static>(
    slots: Arc<Semaphore>,
    cancel: &Cancel,
    deadline: Instant,
    work: impl FnOnce(Cancel) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    check(cancel, deadline)?;
    let until = tokio::time::Instant::from_std(deadline);
    let permit = tokio::select! {
        biased;
        _ = cancel.requested() => return Err(Error::Cancelled),
        _ = tokio::time::sleep_until(until) => return Err(Error::Timeout),
        permit = slots.acquire_owned() => permit.map_err(|_| Error::Helper)?,
    };
    check(cancel, deadline)?;
    let running = Running(Cancel::default());
    let flag = running.0.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        check(&flag, deadline)?;
        work(flag)
    });
    tokio::select! {
        biased;
        _ = cancel.requested() => Err(Error::Cancelled),
        _ = tokio::time::sleep_until(until) => Err(Error::Timeout),
        result = worker => result.map_err(|_| Error::Helper)?,
    }
}

#[cfg(test)]
mod tests;
