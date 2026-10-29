mod capability;
pub(crate) mod streaming;

use super::{Collector, Owner, Readout, Summary, Worker, bootstrap};
use magi_tools::native::Handoff;
pub use magi_tools::native::{Child, Input};
use std::{
    io,
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock, Weak},
};

static CURRENT: OnceLock<Weak<State>> = OnceLock::new();
static READOUT: OnceLock<Readout> = OnceLock::new();
static REQUESTED: AtomicBool = AtomicBool::new(false);
static UNOBSERVED: AtomicU64 = AtomicU64::new(0);

struct State {
    worker: Mutex<Option<Worker>>,
}

/// Own the process collector until all native child owners have been reaped.
pub struct Scope(Arc<State>);

impl Scope {
    pub fn start() -> io::Result<Self> {
        REQUESTED.store(true, Ordering::Release);
        if CURRENT.get().is_some() {
            return Err(io::Error::other("event collection already initialized"));
        }
        if !magi_tools::supplier::launch::install(tool_handoff, tool_context) {
            return Err(io::Error::other("native event handoff already initialized"));
        }
        if !magi_ipc::family::observing::install(tool_context) {
            return Err(io::Error::other("memory event context already initialized"));
        }
        let receiver = crate::events::enable_root()
            .map_err(|_| io::Error::other("event collection unavailable"))?;
        let worker = Worker::start(Collector::new(receiver))?;
        let readout = worker.readout();
        let scope = Self(Arc::new(State {
            worker: Mutex::new(Some(worker)),
        }));
        CURRENT
            .set(Arc::downgrade(&scope.0))
            .map_err(|_| io::Error::other("event collection already initialized"))?;
        let _ = READOUT.set(readout);
        Ok(scope)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Ok(mut worker) = self.0.worker.lock() {
            worker.take();
        }
    }
}

/// Snapshot local counters without enabling collection or waiting for its worker.
pub fn status() -> Status {
    let summary = READOUT.get().map(Readout::snapshot);
    let state = if summary.is_some() {
        StatusKind::Collecting
    } else if REQUESTED.load(Ordering::Acquire) {
        StatusKind::Unavailable
    } else {
        StatusKind::Disabled
    };
    Status {
        state,
        summary,
        unobserved_launches: UNOBSERVED.load(Ordering::Relaxed),
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Status {
    pub state: StatusKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    #[serde(skip_serializing_if = "zero")]
    pub unobserved_launches: u64,
}

fn zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Disabled,
    Unavailable,
    Collecting,
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
        let opened = state
            .worker
            .lock()
            .ok()?
            .as_ref()?
            .open_bound(owner, lineage)
            .ok()?;
        let packet = bootstrap::prepare(opened.sender, input).ok()?;
        Some(Handoff::new(packet, writer, opened.lifetime))
    });
    if handoff.is_none() && REQUESTED.load(Ordering::Acquire) {
        let _ = UNOBSERVED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(1))
        });
    }
    handoff
}
