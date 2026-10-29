mod context;
pub use context::Context;

use std::{
    os::fd::AsFd,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const PEERS: usize = 32;
static CALLBACK: OnceLock<fn() -> Option<Context>> = OnceLock::new();
static OBSERVED: Mutex<Vec<(PathBuf, rustix::process::Pid)>> = Mutex::new(Vec::new());

pub fn install(context: fn() -> Option<Context>) -> bool {
    CALLBACK.set(context).is_ok()
}

pub(super) fn enabled() -> bool {
    CALLBACK.get().is_some()
}

pub(super) fn received(socket: &impl AsFd, path: &Path, verb: &str, reply: &serde_json::Value) {
    if !enabled() || verb != "verbs" || path.as_os_str().len() > 4096 {
        return;
    }
    let Ok(peer) = rustix::net::sockopt::get_socket_peercred(socket) else {
        return;
    };
    if peer.uid != rustix::process::geteuid() {
        return;
    }
    let Ok(mut observed) = OBSERVED.try_lock() else {
        return;
    };
    observed.retain(|(held, _)| held != path);
    if reply
        .get("request_context")
        .and_then(serde_json::Value::as_u64)
        == Some(1)
        && observed.len() < PEERS
    {
        observed.push((path.to_owned(), peer.pid));
    }
}

/// Capture host-owned context only for a socket whose actual peer advertised support.
pub fn capture(socket: &impl AsFd, path: &Path) -> Option<Context> {
    let callback = CALLBACK.get()?;
    let peer = rustix::net::sockopt::get_socket_peercred(socket).ok()?;
    if peer.uid != rustix::process::geteuid() {
        return None;
    }
    let matches = OBSERVED
        .try_lock()
        .ok()?
        .iter()
        .any(|(held, pid)| held == path && *pid == peer.pid);
    if matches { callback() } else { None }
}

pub(super) fn allowance() -> usize {
    if enabled() { 273 } else { 0 }
}

#[cfg(test)]
mod tests;
