//! Whether the work a tool is doing is still wanted.
//!
//! A trait rather than a type, because the thing that knows lives in the daemon and the thing
//! that has to ask lives here. It carries one question and no way to answer it: a tool can
//! find out that it should stop, and cannot stop anything else.
//!
//! Passed to every tool rather than to the ones that might block. A tool that ignores it costs
//! nothing, and a seam only some tools have is one the turn loop has to reason about.

/// A tool's view of the host's interrupt.
pub trait Cancel {
    /// Whether the host has called this work off.
    ///
    /// Asked repeatedly during long work, so it must be cheap and must never block.
    fn is_cancelled(&self) -> bool;
}

/// Nothing is ever called off.
///
/// For callers that have no interrupt to offer: `magi tools` listing what is available, and
/// tests whose subject is not cancellation.
pub struct Uncancelled;

impl Cancel for Uncancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl<T: Cancel + ?Sized> Cancel for &T {
    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stopped;
    impl Cancel for Stopped {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    #[test]
    fn the_default_never_stops_anything() {
        assert!(!Uncancelled.is_cancelled());
    }

    #[test]
    fn a_reference_answers_the_same_as_what_it_points_at() {
        // Tools hold `&dyn Cancel`; without this the borrow would need unwrapping at every
        // call site that already has a reference.
        let stopped = Stopped;
        let by_reference: &dyn Cancel = &stopped;
        assert!(by_reference.is_cancelled());
    }
}
