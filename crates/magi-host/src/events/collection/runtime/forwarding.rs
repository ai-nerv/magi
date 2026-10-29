use super::super::{Forwarder, ShutdownError, branch::Branch, enrollment::Client, watch::Watch};
use crate::events::{Correlation, Fact, Lineage};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) struct Worker {
    pub client: Arc<Client>,
    pub parent: Lineage,
    forwarder: Option<Forwarder>,
    stop: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
    watch: Watch,
}

struct Control(Arc<Client>);

impl Drop for Control {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.close();
        }
    }
}

impl Worker {
    pub(super) fn start(branch: Branch, watch: Watch) -> io::Result<Self> {
        Self::spawn(branch, watch, || {})
    }

    fn spawn(
        branch: Branch,
        watch: Watch,
        begin: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, control, parent) = branch.into_parts();
        let lineage = sender.lineage();
        let client = Arc::new(Client::adopt(control, lineage)?);
        let forwarder =
            Forwarder::activate_watched(sender, watch.clone()).map_err(io::Error::other)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let polling = client.clone();
        let observed = watch.clone();
        let guard = watch.pending(stopping.clone());
        let task = thread::Builder::new()
            .name("magi-event-credit".into())
            .spawn(move || {
                let _guard = guard.enter();
                let _control = Control(polling.clone());
                begin();
                while !stopping.load(Ordering::Acquire) {
                    if observed.failed() {
                        polling.close();
                        break;
                    }
                    if polling.poll().is_err() {
                        observed.fail();
                        break;
                    }
                    if !stopping.load(Ordering::Acquire) {
                        thread::park_timeout(Duration::from_millis(5));
                    }
                }
            })?;
        crate::events::emit(|| (Correlation::default(), Fact::AgentStarted { parent }));
        Ok(Self {
            client,
            parent,
            forwarder: Some(forwarder),
            stop,
            task: Some(task),
            watch,
        })
    }

    pub(super) fn shutdown_started(mut self, started: Instant) -> Result<(), ShutdownError> {
        self.finish(started)
    }

    fn finish(&mut self, started: Instant) -> Result<(), ShutdownError> {
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            task.thread().unpark();
            if task.join().is_err() {
                self.watch.fail();
            }
        }
        let result = self.forwarder.take().map_or(Ok(()), |forwarder| {
            forwarder.shutdown_started(started).map(|_| ())
        });
        if matches!(result, Err(ShutdownError::Panicked)) {
            self.watch.fail();
        }
        self.client.close();
        result
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.task.is_some() || self.forwarder.is_some() {
            let _ = self.finish(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests;
