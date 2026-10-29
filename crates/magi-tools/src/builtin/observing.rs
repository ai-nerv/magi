use std::{os::fd::OwnedFd, sync::OnceLock};

static PREPARE: OnceLock<fn() -> Option<OwnedFd>> = OnceLock::new();

pub fn install(prepare: fn() -> Option<OwnedFd>) -> bool {
    PREPARE.set(prepare).is_ok()
}

pub(super) fn prepare() -> Option<OwnedFd> {
    PREPARE.get().and_then(|prepare| prepare())
}
