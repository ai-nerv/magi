use super::super::{MAX_PRODUCERS, Registrar, branch::Branch, enrollment::Host, watch::Watch};
use crate::events::{Id, Lineage};
use std::{
    io,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle, Thread},
    time::Duration,
};

struct Entry {
    host: Host,
    slots: Arc<AtomicUsize>,
}
impl Entry {
    fn admit(host: Host, slots: &Arc<AtomicUsize>) -> Option<Self> {
        slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_PRODUCERS).then_some(n + 1)
            })
            .ok()?;
        Some(Self {
            host,
            slots: slots.clone(),
        })
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.slots.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub(super) struct Admission {
    registrar: Registrar,
    slots: Arc<AtomicUsize>,
    pending: Arc<RwLock<mpsc::SyncSender<Entry>>>,
    stop: Arc<AtomicBool>,
    wake: Thread,
}

pub(super) struct Worker {
    admission: Admission,
    task: Option<JoinHandle<()>>,
    watch: Watch,
}

impl Worker {
    pub(super) fn start(registrar: Registrar, watch: Watch) -> io::Result<Self> {
        Self::spawn(registrar, watch, || {})
    }

    fn spawn(
        registrar: Registrar,
        watch: Watch,
        begin: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(MAX_PRODUCERS);
        let stop = Arc::new(AtomicBool::new(false));
        let slots = Arc::new(AtomicUsize::new(0));
        let active = slots.clone();
        let registry = registrar.clone();
        let stopping = stop.clone();
        let guard = watch.pending(stopping.clone());
        let task = thread::Builder::new()
            .name("magi-event-tree".into())
            .spawn(move || {
                let _guard = guard.enter();
                begin();
                let mut hosts: Vec<Entry> = Vec::new();
                while !stopping.load(Ordering::Acquire) {
                    for _ in 0..MAX_PRODUCERS {
                        let Ok(entry) = receiver.try_recv() else {
                            break;
                        };
                        hosts.push(entry);
                    }
                    let mut nested = Vec::new();
                    hosts.retain_mut(|entry| {
                        let alive = entry
                            .host
                            .poll_branches(active.load(Ordering::Acquire) < MAX_PRODUCERS);
                        if let Some(host) = entry.host.take_branch() {
                            if let Some(child) = Entry::admit(host, &active) {
                                nested.push(child);
                            } else {
                                registry.refuse();
                            }
                        }
                        alive
                    });
                    hosts.extend(nested);
                    if !stopping.load(Ordering::Acquire) {
                        thread::park_timeout(Duration::from_millis(5));
                    }
                }
            })?;
        let admission = Admission {
            registrar,
            slots,
            pending: Arc::new(RwLock::new(sender)),
            stop,
            wake: task.thread().clone(),
        };
        Ok(Self {
            admission,
            task: Some(task),
            watch,
        })
    }

    pub(super) fn admission(&self) -> Admission {
        self.admission.clone()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        {
            let _pending = self.admission.pending.write();
            self.admission.stop.store(true, Ordering::Release);
        }
        self.admission.wake.unpark();
        if let Some(task) = self.task.take()
            && task.join().is_err()
        {
            self.watch.fail();
        }
    }
}

#[cfg(test)]
mod tests;

impl Admission {
    pub(super) fn prepare(&self, parent: Lineage) -> Option<Branch> {
        if self.stop.load(Ordering::Acquire) {
            return None;
        }
        let lineage = Lineage::root(parent.run_id?, Id::generate().ok()?);
        let (host, sender, control) = Host::pair(self.registrar.clone(), lineage).ok()?;
        let entry = Entry::admit(host, &self.slots)?;
        let branch = Branch::new(sender, control, parent).ok()?;
        let pending = self.pending.try_read().ok()?;
        if self.stop.load(Ordering::Acquire) {
            return None;
        }
        pending.try_send(entry).ok()?;
        self.wake.unpark();
        Some(branch)
    }
}
