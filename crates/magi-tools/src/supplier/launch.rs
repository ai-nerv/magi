use crate::native::{self, Child, Handoff};
use std::{io, process::Command, sync::OnceLock};

pub use magi_ipc::family::observing::Context;

#[derive(Clone, Copy)]
pub enum Kind {
    Call,
    Daemon,
}

type Prepare = fn(&Command, &dyn crate::Cancel, Kind) -> Option<Handoff>;
struct Callbacks {
    prepare: Prepare,
    context: fn() -> Option<Context>,
}
static CALLBACKS: OnceLock<Callbacks> = OnceLock::new();

/// Install the coordinator's optional native run/serve handoff.
pub fn install(prepare: Prepare, context: fn() -> Option<Context>) -> bool {
    CALLBACKS.set(Callbacks { prepare, context }).is_ok()
}

pub(super) fn spawn(command: Command) -> io::Result<Child> {
    let handoff = prepare(&command, &crate::Uncancelled, Kind::Daemon);
    native::spawn(command, handoff)
}

pub(super) fn prepare(
    command: &Command,
    cancel: &dyn crate::Cancel,
    kind: Kind,
) -> Option<Handoff> {
    CALLBACKS
        .get()
        .and_then(|callbacks| (callbacks.prepare)(command, cancel, kind))
}

pub(super) fn context() -> Option<Context> {
    (CALLBACKS.get()?.context)()
}
