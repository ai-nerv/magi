//! The turn loop, as an explicit state machine.
//!
//! No filesystem, no terminal, no host, no HTTP. A turn is folded from provider deltas and
//! answers one question — what should happen next — so the whole loop is testable without a
//! model, and the daemon that drives it holds no agent logic of its own.
//!
//! Pi's equivalent is two nested `while` loops, which is why "am I steering or following up"
//! is answered by which loop you are in and resumability after a crash is undefined. An enum
//! costs a little more to write and can be asked what it is doing.

mod turn;

pub use turn::{PendingCall, Step, Turn, TurnState};
