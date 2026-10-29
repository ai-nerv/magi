mod branches;
mod capability;
mod forwarding;
pub(crate) mod streaming;

use super::{
    Collector, Owner, Readout, Registrar, Summary, Worker, bootstrap, branch::Branch,
    enrollment::Client, watch::Watch,
};
use magi_tools::native::Handoff;
pub use magi_tools::native::{Child, Input};
use std::{
    io,
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Instant,
};

static CURRENT: OnceLock<Weak<State>> = OnceLock::new();
static READOUT: OnceLock<Readout> = OnceLock::new();
static REQUESTED: AtomicBool = AtomicBool::new(false);
static UNOBSERVED: AtomicU64 = AtomicU64::new(0);
static PARENT: OnceLock<crate::events::Lineage> = OnceLock::new();
static OVERRUN: AtomicBool = AtomicBool::new(false);
static WATCH: OnceLock<Watch> = OnceLock::new();

struct State {
    worker: Mutex<Option<Observation>>,
    admission: Admission,
}

enum Admission {
    Root {
        registrar: Registrar,
        branches: branches::Admission,
    },
    Child(Arc<Client>),
}

enum Observation {
    Root {
        worker: Worker,
        branches: branches::Worker,
    },
    Child(forwarding::Worker),
}

fn install() -> io::Result<()> {
    REQUESTED.store(true, Ordering::Release);
    if CURRENT.get().is_some() {
        return Err(io::Error::other("event collection already initialized"));
    }
    if !magi_tools::supplier::launch::install(tool_handoff, tool_context)
        || !magi_ipc::family::observing::install(tool_context)
        || !magi_tools::builtin::observing::install(prepare_child)
    {
        return Err(io::Error::other("native event handoff already initialized"));
    }
    Ok(())
}

/// Own the process collector until all native child owners have been reaped.
pub struct Scope(Arc<State>);

impl Scope {
    pub fn start() -> io::Result<Self> {
        install()?;
        let receiver = crate::events::enable_root()
            .map_err(|_| io::Error::other("event collection unavailable"))?;
        let worker = Worker::start(Collector::new(receiver))?;
        let readout = worker.readout();
        let watch = Watch::default();
        let branches = branches::Worker::start(worker.registrar(), watch.clone())?;
        let scope = Self(Arc::new(State {
            admission: Admission::Root {
                registrar: worker.registrar(),
                branches: branches.admission(),
            },
            worker: Mutex::new(Some(Observation::Root { worker, branches })),
        }));
        CURRENT
            .set(Arc::downgrade(&scope.0))
            .map_err(|_| io::Error::other("event collection already initialized"))?;
        let _ = READOUT.set(readout);
        let _ = WATCH.set(watch);
        Ok(scope)
    }

    pub fn from_child(branch: Branch) -> io::Result<Self> {
        install()?;
        let watch = Watch::default();
        let worker = forwarding::Worker::start(branch, watch.clone())?;
        let _ = PARENT.set(worker.parent);
        let scope = Self(Arc::new(State {
            admission: Admission::Child(worker.client.clone()),
            worker: Mutex::new(Some(Observation::Child(worker))),
        }));
        CURRENT
            .set(Arc::downgrade(&scope.0))
            .map_err(|_| io::Error::other("event collection already initialized"))?;
        let _ = WATCH.set(watch);
        Ok(scope)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let start = Instant::now();
        let observation = self
            .0
            .worker
            .lock()
            .ok()
            .and_then(|mut worker| worker.take());
        match observation {
            Some(Observation::Root { worker, branches }) => {
                drop(branches);
                let _ = worker.shutdown_started(start);
            }
            Some(Observation::Child(worker)) => {
                let _ = worker.shutdown_started(start);
            }
            None => {}
        }
        if start.elapsed() > super::SHUTDOWN_BUDGET {
            OVERRUN.store(true, Ordering::Release);
        }
    }
}

/// Snapshot local counters without enabling collection or waiting for its worker.
pub fn status() -> Status {
    let failed = WATCH.get().is_some_and(Watch::failed);
    let summary = READOUT.get().map(|readout| {
        let mut summary = readout.snapshot();
        summary.worker_failed |= failed;
        summary.shutdown_overrun |= OVERRUN.load(Ordering::Acquire);
        summary
    });
    let state = if summary.is_some() {
        StatusKind::Collecting
    } else if !failed
        && CURRENT.get().and_then(Weak::upgrade).is_some_and(
            |state| matches!(&state.admission, Admission::Child(client) if !client.is_closed()),
        )
    {
        StatusKind::Forwarding
    } else if REQUESTED.load(Ordering::Acquire) {
        StatusKind::Unavailable
    } else {
        StatusKind::Disabled
    };
    let worker_failed = failed || summary.is_some_and(|summary| summary.worker_failed);
    Status {
        state,
        summary,
        unobserved_launches: UNOBSERVED.load(Ordering::Relaxed),
        lineage: REQUESTED
            .load(Ordering::Acquire)
            .then(crate::events::process_lineage),
        parent: PARENT.get().copied(),
        shutdown_overrun: OVERRUN.load(Ordering::Acquire),
        worker_failed,
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Status {
    pub state: StatusKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    #[serde(skip_serializing_if = "zero")]
    pub unobserved_launches: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineage: Option<crate::events::Lineage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<crate::events::Lineage>,
    #[serde(skip_serializing_if = "is_false")]
    pub shutdown_overrun: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub worker_failed: bool,
}

fn zero(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Disabled,
    Unavailable,
    Collecting,
    Forwarding,
}

/// Spawn a selected native role with piped input, optionally handing off events.
pub fn spawn(command: Command, owner: Owner) -> io::Result<Child> {
    let (command, supported) = negotiate(command, owner);
    prepare(command, owner, supported, crate::events::process_lineage()).spawn()
}

struct Prepared {
    command: Command,
    handoff: Option<Handoff>,
}

impl Prepared {
    fn spawn(self) -> io::Result<Child> {
        magi_tools::native::spawn(self.command, self.handoff)
    }
}

fn negotiate(command: Command, owner: Owner) -> (Command, bool) {
    let supported =
        CURRENT.get().and_then(Weak::upgrade).is_some() && capability::supported(&command, owner);
    (command, supported)
}

fn prepare(
    mut command: Command,
    owner: Owner,
    supported: bool,
    lineage: crate::events::Lineage,
) -> Prepared {
    command.stdin(Stdio::piped());
    Prepared {
        command,
        handoff: handoff(owner, supported, lineage),
    }
}

fn tool_handoff(
    command: &Command,
    cancel: &dyn magi_tools::Cancel,
    kind: magi_tools::supplier::launch::Kind,
) -> Option<Handoff> {
    let supported = CURRENT.get().and_then(Weak::upgrade).is_some()
        && capability::supported(command, Owner::Casper);
    if cancel.is_cancelled() {
        return None;
    }
    let lineage = match kind {
        magi_tools::supplier::launch::Kind::Call => crate::events::context::lineage(),
        magi_tools::supplier::launch::Kind::Daemon => crate::events::process_lineage(),
    };
    handoff(Owner::Casper, supported, lineage)
}

fn tool_context() -> Option<magi_tools::supplier::launch::Context> {
    let _state = CURRENT.get()?.upgrade()?;
    let lineage = crate::events::context::lineage();
    magi_tools::supplier::launch::Context::new(
        String::from(lineage.run_id?),
        String::from(lineage.agent_id?),
        lineage.turn_id.map(String::from),
        lineage.ask_id.map(String::from),
    )
}

fn handoff(owner: Owner, supported: bool, lineage: crate::events::Lineage) -> Option<Handoff> {
    let handoff = CURRENT.get().and_then(Weak::upgrade).and_then(|state| {
        if !supported {
            return None;
        }
        let (input, writer) = UnixStream::pair().ok()?;
        match &state.admission {
            Admission::Root { registrar, .. } => {
                let opened = registrar.open_bound(owner, lineage).ok()?;
                let packet = bootstrap::prepare(opened.sender, input).ok()?;
                Some(Handoff::new(packet, writer, opened.lifetime))
            }
            Admission::Child(client) => {
                let opened = client.open(owner, lineage).ok()?;
                let packet = bootstrap::prepare(opened.sender, input).ok()?;
                Some(Handoff::new(packet, writer, opened.lifetime))
            }
        }
    });
    if handoff.is_none() && REQUESTED.load(Ordering::Acquire) {
        let _ = UNOBSERVED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(1))
        });
    }
    handoff
}

fn prepare_child() -> Option<std::os::fd::OwnedFd> {
    let result = (|| {
        let state = CURRENT.get()?.upgrade()?;
        let parent = crate::events::context::lineage();
        let branch = match &state.admission {
            Admission::Root { branches, .. } => branches.prepare(parent)?,
            Admission::Child(client) => client.branch(parent).ok()?,
        };
        branch.prepare(std::fs::File::open("/dev/null").ok()?).ok()
    })();
    if result.is_none() && REQUESTED.load(Ordering::Acquire) {
        let _ = UNOBSERVED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(1))
        });
    }
    result
}

pub fn unobserved_child() {
    REQUESTED.store(true, Ordering::Release);
    let _ = UNOBSERVED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_add(1))
    });
}

#[cfg(test)]
mod tests;
