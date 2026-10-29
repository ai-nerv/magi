use super::super::{Correlation, Id, MAX_EVENT_BYTES, Revision};
use serde::Deserialize;
use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    Magi,
    Melchior,
    Casper,
    Balthasar,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<F> {
    schema: Revision,
    program: Owner,
    program_version: [u16; 3],
    producer: Id,
    sequence: NonZeroU64,
    correlation: Correlation,
    fact: F,
}

pub(super) struct Parsed {
    pub producer: Id,
    pub sequence: NonZeroU64,
    pub correlation: Correlation,
    pub usage: Option<Usage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Oversized,
    Schema,
    Owner,
    Producer,
}

fn parse<F: serde::de::DeserializeOwned>(
    bytes: &[u8],
    owner: Owner,
    producer: Id,
    usage: impl FnOnce(F) -> Option<Usage>,
) -> Result<Parsed, Invalid> {
    let event: Envelope<F> = serde_json::from_slice(bytes).map_err(|_| Invalid::Schema)?;
    if event.program != owner {
        return Err(Invalid::Owner);
    }
    if event.producer != producer {
        return Err(Invalid::Producer);
    }
    let _ = (event.schema, event.program_version);
    Ok(Parsed {
        producer: event.producer,
        sequence: event.sequence,
        correlation: event.correlation,
        usage: usage(event.fact),
    })
}

pub(super) fn decode(bytes: &[u8], owner: Owner, producer: Id) -> Result<Parsed, Invalid> {
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(Invalid::Oversized);
    }
    match owner {
        Owner::Magi => parse::<super::super::Fact>(bytes, owner, producer, |_| None),
        Owner::Melchior => parse::<ModelFact>(bytes, owner, producer, |fact| match fact {
            ModelFact::UsageSnapshot(usage) => Some(usage),
            ModelFact::ProviderAttempt {
                outcome,
                duration_ms,
            } => {
                let _ = (outcome, duration_ms);
                None
            }
            ModelFact::Retry { outcome } => {
                let _ = outcome;
                None
            }
        }),
        Owner::Casper => parse::<Tools>(bytes, owner, producer, |fact| {
            match fact {
                Tools::Tool {
                    outcome,
                    duration_ms,
                } => {
                    let _ = (outcome, duration_ms);
                }
                Tools::Mutation { state } => match state {
                    Mutation::Committed {
                        durability,
                        observation,
                        delivery,
                    } => {
                        let _ = (durability, observation, delivery);
                    }
                    Mutation::Refused {} | Mutation::Unknown {} => {}
                },
                Tools::ToolServer { outcome } => {
                    let _ = outcome;
                }
            }
            None
        }),
        Owner::Balthasar => parse::<StoreFact>(bytes, owner, producer, |fact| {
            match fact {
                StoreFact::Layout {
                    outcome,
                    duration_ms,
                } => {
                    let _ = (outcome, duration_ms);
                }
                StoreFact::JobResult { outcome, revision } => {
                    let _ = (outcome, revision);
                }
                StoreFact::Memory { outcome } => {
                    let _ = outcome;
                }
            }
            None
        }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub revision: NonZeroU64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reported_cost_micros: Option<u64>,
    pub estimated_cost_micros: Option<u64>,
    pub ledger: Ledger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ledger {
    Persisted,
    Unchanged,
    Untracked,
    Failed,
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ModelFact {
    ProviderAttempt {
        outcome: Attempt,
        duration_ms: Option<u64>,
    },
    Retry {
        outcome: Retry,
    },
    UsageSnapshot(Usage),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Attempt {
    Dispatched,
    Succeeded,
    Cancelled,
    Failed,
    Refused,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Retry {
    Scheduled,
    Dispatched,
    Cancelled,
    Failed,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Tools {
    Tool {
        outcome: Tool,
        duration_ms: Option<u64>,
    },
    Mutation {
        state: Mutation,
    },
    ToolServer {
        outcome: Server,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Tool {
    Admitted,
    Started,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
    Refused,
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Mutation {
    Committed {
        durability: Confirmation,
        observation: Confirmation,
        delivery: Delivery,
    },
    Refused {},
    Unknown {},
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Confirmation {
    Confirmed,
    Unconfirmed,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Delivery {
    Reported,
    Unconfirmed,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Server {
    Connecting,
    Connected,
    CloseRequested,
    Disconnected,
    AuthenticationRequired,
    Refused,
    Failed,
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoreFact {
    Layout {
        outcome: Layout,
        duration_ms: Option<u64>,
    },
    JobResult {
        outcome: Job,
        revision: Option<NonZeroU64>,
    },
    Memory {
        outcome: Operation,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Layout {
    Decided,
    Applied,
    ApplicationFailed,
    IgnoredRepeated,
    Recovered,
    RecoveryFailed,
    Failed,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Job {
    Accepted,
    RetryQueued,
    Failed,
    IgnoredObsolete,
    IgnoredRepeated,
    Rejected,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Read,
    Written,
    Failed,
}
