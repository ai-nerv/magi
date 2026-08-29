//! The user's interrupt, as something a running turn can see.
//!
//! An interrupt arrives on a connection task and has to stop work happening on the worker
//! thread, so it cannot be a return value or an error — it is shared state both sides hold.
//! Two halves, because a turn waits in two different ways: a flag for the loop that checks
//! between steps, and a notification for the provider call that would otherwise block until
//! the model was finished having its say.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// A shared interrupt.
#[derive(Clone, Default)]
pub struct Cancel {
    requested: Arc<AtomicBool>,
    woken: Arc<Notify>,
}

impl Cancel {
    /// Ask the running turn to stop.
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.woken.notify_waiters();
    }

    /// Clear the request, so the next turn starts uninterrupted.
    ///
    /// An interrupt that outlived the turn it was meant for would cancel the prompt typed to
    /// replace it, which reads as a session that has stopped accepting input.
    pub fn clear(&self) {
        self.requested.store(false, Ordering::SeqCst);
    }

    /// Whether a stop has been asked for.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Resolve when a stop is asked for.
    ///
    /// Checks the flag first: a request that landed before the wait began has no notification
    /// left to deliver, and waiting for one would sleep until the turn ended on its own.
    pub async fn requested(&self) {
        if self.is_requested() {
            return;
        }
        self.woken.notified().await;
    }
}

/// A running tool asks the same question the turn loop does.
///
/// The daemon owns the interrupt and `axon-tools` owns the tools, so the two meet at a trait
/// with one method. A tool can find out that it should stop, and can do nothing else with it.
impl axon_tools::Cancel for Cancel {
    fn is_cancelled(&self) -> bool {
        self.is_requested()
    }
}

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cancel")
            .field("requested", &self.is_requested())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_request_that_arrives_first_still_resolves_the_wait() {
        let cancel = Cancel::default();
        cancel.request();
        // Would hang on the notification alone: `notify_waiters` reaches nobody.
        tokio::time::timeout(std::time::Duration::from_millis(50), cancel.requested())
            .await
            .expect("the wait sees the flag");
    }

    #[tokio::test]
    async fn a_request_that_arrives_during_the_wait_wakes_it() {
        let cancel = Cancel::default();
        let waiting = cancel.clone();
        let task = tokio::spawn(async move { waiting.requested().await });
        tokio::task::yield_now().await;
        cancel.request();
        tokio::time::timeout(std::time::Duration::from_millis(200), task)
            .await
            .expect("woken")
            .expect("joined");
    }

    #[test]
    fn clearing_lets_the_next_turn_run() {
        let cancel = Cancel::default();
        cancel.request();
        assert!(cancel.is_requested());
        cancel.clear();
        assert!(!cancel.is_requested());
    }

    #[test]
    fn clones_share_one_request() {
        let cancel = Cancel::default();
        cancel.clone().request();
        assert!(cancel.is_requested());
    }
}
