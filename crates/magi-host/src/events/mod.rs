pub mod collection;
pub(crate) mod context;
mod lineage;
mod schema;
pub use lineage::Lineage;
pub use schema::*;

use std::{
    io::Write,
    num::NonZeroU64,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
};

pub const MAX_EVENT_BYTES: usize = 4096;
pub const QUEUED_EVENTS: usize = 256;
static PROCESS: OnceLock<Emitter> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emitted {
    Disabled,
    Queued,
    Full,
    Closed,
    Encoding,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnableError {
    AlreadyEnabled,
    Entropy,
    Version,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loss {
    pub abandoned: u64,
    pub full: u64,
    pub closed: u64,
    pub encoding: u64,
    pub exhausted: u64,
}

#[derive(Default)]
struct Counters {
    active: AtomicU64,
    enqueued: AtomicU64,
    dequeued: AtomicU64,
    receiver_closed: AtomicBool,
    full: AtomicU64,
    closed: AtomicU64,
    encoding: AtomicU64,
    exhausted: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> Loss {
        Loss {
            abandoned: if self.receiver_closed.load(Ordering::Acquire) {
                self.enqueued
                    .load(Ordering::Relaxed)
                    .saturating_sub(self.dequeued.load(Ordering::Relaxed))
            } else {
                0
            },
            full: self.full.load(Ordering::Relaxed),
            closed: self.closed.load(Ordering::Relaxed),
            encoding: self.encoding.load(Ordering::Relaxed),
            exhausted: self.exhausted.load(Ordering::Relaxed),
        }
    }
}

pub struct Receiver {
    inner: mpsc::Receiver<Vec<u8>>,
    loss: Arc<Counters>,
    producer: Id,
    lineage: Lineage,
}

impl Receiver {
    pub fn try_recv(&self) -> Result<Vec<u8>, mpsc::TryRecvError> {
        let bytes = self.inner.try_recv()?;
        self.loss.dequeued.fetch_add(1, Ordering::Relaxed);
        Ok(bytes)
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.loss.receiver_closed.store(true, Ordering::SeqCst);
    }
}

struct Emitter {
    producer: Id,
    lineage: Lineage,
    version: [u16; 3],
    sequence: AtomicU64,
    identities: AtomicU64,
    sender: SyncSender<Vec<u8>>,
    loss: Arc<Counters>,
}

impl Emitter {
    fn new(producer: Id, version: [u16; 3]) -> (Self, Receiver) {
        let (sender, receiver) = mpsc::sync_channel(QUEUED_EVENTS);
        let loss = Arc::new(Counters::default());
        (
            Self {
                producer,
                lineage: Lineage::default(),
                version,
                sequence: AtomicU64::new(1),
                identities: AtomicU64::new(1),
                sender,
                loss: loss.clone(),
            },
            Receiver {
                inner: receiver,
                loss,
                producer,
                lineage: Lineage::default(),
            },
        )
    }

    fn identity(&self) -> Option<Id> {
        match self
            .identities
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        {
            Ok(counter) => Some(Id::local(self.producer, counter)),
            Err(_) => {
                dropped(&self.loss.exhausted, Emitted::Exhausted);
                None
            }
        }
    }

    fn emit(&self, prepare: impl FnOnce() -> (Correlation, Fact)) -> Emitted {
        if self
            .loss
            .active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .is_err()
        {
            return dropped(&self.loss.exhausted, Emitted::Exhausted);
        }
        let _active = Active {
            counters: &self.loss,
            unwinding: std::thread::panicking(),
        };
        let sequence = self
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1));
        let Ok(sequence) = sequence else {
            return dropped(&self.loss.exhausted, Emitted::Exhausted);
        };
        let Some(sequence) = NonZeroU64::new(sequence) else {
            return dropped(&self.loss.exhausted, Emitted::Exhausted);
        };
        if self.loss.receiver_closed.load(Ordering::SeqCst) {
            return dropped(&self.loss.closed, Emitted::Closed);
        }
        let (correlation, fact) = prepare();
        let event = Event {
            schema: Revision,
            program: Program::Magi,
            program_version: self.version,
            producer: self.producer,
            sequence,
            correlation: context::fill(self.lineage.apply(correlation)),
            fact,
        };
        let mut encoded = Encoded {
            bytes: [0; MAX_EVENT_BYTES],
            len: 0,
        };
        if serde_json::to_writer(&mut encoded, &event).is_err() {
            return dropped(&self.loss.encoding, Emitted::Encoding);
        }
        let Some(bytes) = encoded.bytes.get(..encoded.len) else {
            return dropped(&self.loss.encoding, Emitted::Encoding);
        };
        match self.sender.try_send(bytes.to_vec()) {
            Ok(()) => {
                self.loss.enqueued.fetch_add(1, Ordering::Relaxed);
                Emitted::Queued
            }
            Err(TrySendError::Full(_)) => dropped(&self.loss.full, Emitted::Full),
            Err(TrySendError::Disconnected(_)) => dropped(&self.loss.closed, Emitted::Closed),
        }
    }
}

struct Active<'a> {
    counters: &'a Counters,
    unwinding: bool,
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        if !self.unwinding && std::thread::panicking() {
            dropped(&self.counters.encoding, Emitted::Encoding);
        }
        self.counters.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn dropped(counter: &AtomicU64, result: Emitted) -> Emitted {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_add(1))
    });
    result
}

struct Encoded {
    bytes: [u8; MAX_EVENT_BYTES],
    len: usize,
}

impl Write for Encoded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let output = self
            .bytes
            .get_mut(self.len..)
            .and_then(|tail| tail.get_mut(..bytes.len()))
            .ok_or(std::io::ErrorKind::FileTooLarge)?;
        output.copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn enable_local() -> Result<Receiver, EnableError> {
    if PROCESS.get().is_some() {
        return Err(EnableError::AlreadyEnabled);
    }
    enable_bound(
        Id::generate().map_err(|_| EnableError::Entropy)?,
        Lineage::default(),
    )
}

fn enable_bound(producer: Id, lineage: Lineage) -> Result<Receiver, EnableError> {
    if PROCESS.get().is_some() {
        return Err(EnableError::AlreadyEnabled);
    }
    let mut version = [0; 3];
    for (slot, value) in version.iter_mut().zip([
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"),
    ]) {
        *slot = value.parse().map_err(|_| EnableError::Version)?;
    }
    let (mut emitter, mut receiver) = Emitter::new(producer, version);
    emitter.lineage = lineage;
    receiver.lineage = lineage;
    PROCESS
        .set(emitter)
        .map_err(|_| EnableError::AlreadyEnabled)?;
    Ok(receiver)
}

pub(crate) fn enable_root() -> Result<Receiver, EnableError> {
    let id = || Id::generate().map_err(|_| EnableError::Entropy);
    enable_bound(id()?, Lineage::root(id()?, id()?))
}

pub(crate) fn process_lineage() -> Lineage {
    PROCESS
        .get()
        .map_or_else(Lineage::default, |emitter| emitter.lineage)
}

pub fn emit(prepare: impl FnOnce() -> (Correlation, Fact)) -> Emitted {
    PROCESS
        .get()
        .map_or(Emitted::Disabled, |emitter| emitter.emit(prepare))
}

pub(crate) fn identity() -> Option<Id> {
    PROCESS.get()?.identity()
}

pub fn loss() -> Loss {
    PROCESS
        .get()
        .map_or_else(Loss::default, |emitter| emitter.loss.snapshot())
}

#[cfg(test)]
mod tests;
