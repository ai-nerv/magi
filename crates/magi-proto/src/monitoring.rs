//! Read-only session details, without a transcript or event subscription.

use crate::{AgentStatus, ModelInfo};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDetails {
    pub model: Option<ModelInfo>,
    pub thinking: String,
    pub status: AgentStatus,
    pub approvals: usize,
    pub questions: usize,
    pub context: Option<RequestContext>,
    pub cost_micros: u64,
}

/// The last admitted request's complete input, not cumulative session usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestContext {
    pub input: u64,
    pub capacity: u64,
    pub limit: u64,
    pub estimated: bool,
}
