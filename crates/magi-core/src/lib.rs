//! The turn loop as an explicit state machine: a turn is folded from provider deltas and answers
//! one question — what should happen next — so it is testable without a model, filesystem,
//! terminal or HTTP.

mod turn;

pub use turn::{PendingCall, Step, Turn, TurnState};
