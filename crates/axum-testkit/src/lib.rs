//! A harness that isn't one.
//!
//! Serves a recorded event stream over a real Unix socket so the UI can be developed against
//! a file instead of a model. The transport is production code; only the source of events is
//! fake, which is what makes this useful rather than a mock.

pub mod conformance;
pub mod replay;

pub use replay::{FakeHarness, Recording};
