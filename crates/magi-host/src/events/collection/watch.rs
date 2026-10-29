use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub(super) struct Watch(Arc<AtomicBool>);

pub(super) struct Guard {
    watch: Watch,
    stop: Arc<AtomicBool>,
    entered: bool,
}

impl Watch {
    pub(super) fn failed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(super) fn fail(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(super) fn pending(&self, stop: Arc<AtomicBool>) -> Guard {
        Guard {
            watch: self.clone(),
            stop,
            entered: false,
        }
    }
}

impl Guard {
    pub(super) fn enter(mut self) -> Self {
        self.entered = true;
        self
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.entered || std::thread::panicking() {
            self.stop.store(true, Ordering::Release);
            self.watch.fail();
        }
    }
}

#[cfg(test)]
mod tests;
