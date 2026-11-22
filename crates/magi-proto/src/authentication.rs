//! Ephemeral provider-authentication control messages.

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Request {
    List,
    SetKey { provider: String, key: String },
    Login { provider: String },
    Remove { provider: String },
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::List => "List",
            Self::SetKey { .. } => "SetKey([redacted])",
            Self::Login { .. } => "Login",
            Self::Remove { .. } => "Remove",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub method: String,
    pub status: String,
    pub source: String,
    pub saved: bool,
    pub available: bool,
    pub variables: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Reply {
    Device {
        url: String,
        code: String,
        expires_in: u64,
    },
    Browser {
        url: String,
    },
    Finished {
        providers: Vec<Account>,
        error: Option<String>,
    },
}

impl Reply {
    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Finished {
            providers: Vec::new(),
            error: Some(message.into()),
        }
    }
}
