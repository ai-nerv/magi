//! Signing in to a subscription, rather than exporting a key.
//!
//! An API key is a string a person can put in their environment. A subscription is not: the
//! only way to hold one is a token you were given, that expires, and that has to be renewed
//! without asking again. That is the whole of the difference, and the whole of why this exists.
//!
//! **What is stored, and where.** `$XDG_DATA_HOME/axon/credentials.json`, mode `0600`, one
//! entry per provider. Not the system keyring: a keyring is a second daemon, a second failure
//! mode and a second thing to explain, and a file only the user can read is what every other
//! tool on the machine already uses for the same job. Never the journal — a transcript is
//! meant to be readable, copyable and shareable, which is exactly what a token must not be.

mod flow;

pub use flow::{Pkce, authorize_url, exchange, listen_for_code};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What a provider gave back, and when it stops working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// Sent as a bearer token.
    pub access: String,
    /// Exchanged for a new access token when that one expires.
    ///
    /// Optional because not every provider issues one. Without it, an expiry means signing in
    /// again, which is worse but is the provider's choice rather than ours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// Unix seconds after which `access` is no longer accepted.
    pub expires_at: u64,
}

/// How long before the stated expiry a token is treated as already expired.
///
/// A token that expires while the request carrying it is in flight fails as a 401, and the
/// turn is lost for a reason that had nothing to do with the conversation. Renewing early
/// costs one extra exchange and removes the race.
const EARLY: u64 = 60;

impl Tokens {
    /// Whether this should be renewed before it is used.
    #[must_use]
    pub fn is_stale(&self, now: u64) -> bool {
        self.expires_at <= now.saturating_add(EARLY)
    }
}

/// Every provider a person has signed in to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    /// Keyed by provider id, which is what a config and a model name already use.
    #[serde(default)]
    pub providers: BTreeMap<String, Tokens>,
}

impl Store {
    /// Read the store, or an empty one if there is none.
    ///
    /// A missing file is the normal case and not an error. A *corrupt* one is returned as an
    /// error rather than silently replaced: overwriting it would sign the user out of
    /// everything to recover from what may be a bad disk.
    pub fn load() -> Result<Self, Error> {
        Self::load_from(&path())
    }

    /// Read a store from a named file.
    ///
    /// Named rather than assumed, so the location is an argument and not a global read at
    /// depth. Tests use it; so would a second profile, if there is ever one.
    pub fn load_from(path: &std::path::Path) -> Result<Self, Error> {
        match std::fs::read_to_string(path) {
            Ok(source) => serde_json::from_str(&source).map_err(|e| Error::Corrupt {
                path: path.to_owned(),
                detail: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Write the store back, readable only by its owner.
    ///
    /// The mode is set before the content is written, not after: a token that is briefly
    /// world-readable has been readable, and nothing later takes that back.
    pub fn save(&self) -> Result<(), Error> {
        self.save_to(&path())
    }

    /// Write a store to a named file, readable only by its owner.
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), Error> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    /// The tokens for one provider.
    #[must_use]
    pub fn get(&self, provider: &str) -> Option<&Tokens> {
        self.providers.get(provider)
    }

    /// Record a sign-in.
    pub fn put(&mut self, provider: &str, tokens: Tokens) {
        self.providers.insert(provider.to_owned(), tokens);
    }

    /// Forget one provider. Returns whether there was anything to forget.
    pub fn forget(&mut self, provider: &str) -> bool {
        self.providers.remove(provider).is_some()
    }
}

/// Where credentials live.
///
/// Beside the sessions rather than in the config directory: this is state the tool produced,
/// not configuration a person wrote, and a config directory people copy between machines is
/// the last place a token should be.
#[must_use]
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("axon").join("credentials.json")
}

/// Seconds since the epoch.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Anything that can go wrong holding a credential.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The store could not be read or written.
    #[error("credentials: {0}")]
    Io(#[from] std::io::Error),

    /// The store is not valid JSON.
    ///
    /// Reported rather than repaired: replacing it signs the user out of everything, which is
    /// a heavy answer to what may be a disk that needs looking at.
    #[error("{path} is not readable as credentials ({detail}); move it aside to start again")]
    Corrupt {
        /// Where the unreadable file is.
        path: PathBuf,
        /// What the parser objected to.
        detail: String,
    },

    /// The store could not be serialised.
    #[error("credentials: {0}")]
    Encode(#[from] serde_json::Error),

    /// The provider refused.
    #[error("{0}")]
    Refused(String),

    /// There is nothing stored for this provider.
    #[error("not signed in to {0}; run `axon auth login {0}`")]
    NotSignedIn(String),

    /// The provider issued no refresh token, and the access token has expired.
    #[error("the session for {0} expired; run `axon auth login {0}` again")]
    Expired(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(expires_at: u64) -> Tokens {
        Tokens {
            access: "at".into(),
            refresh: Some("rt".into()),
            expires_at,
        }
    }

    #[test]
    fn a_token_is_stale_before_it_actually_expires() {
        // Otherwise it can expire while the request carrying it is in flight, and the turn is
        // lost for a reason that had nothing to do with the conversation.
        assert!(tokens(1000).is_stale(1000 - EARLY));
        assert!(!tokens(1000).is_stale(1000 - EARLY - 1));
    }

    #[test]
    fn an_expired_token_is_stale() {
        assert!(tokens(500).is_stale(1000));
    }

    #[test]
    fn credentials_live_beside_the_sessions_not_the_config() {
        // A config directory is the sort of thing people copy between machines.
        let path = path();
        assert!(
            path.ends_with("axon/credentials.json"),
            "{}",
            path.display()
        );
        assert!(
            !path.to_string_lossy().contains(".config"),
            "{}",
            path.display()
        );
    }

    #[test]
    fn a_store_round_trips() {
        let mut store = Store::default();
        store.put("anthropic", tokens(1000));
        let json = serde_json::to_string(&store).expect("encode");
        let back: Store = serde_json::from_str(&json).expect("decode");
        assert_eq!(back.get("anthropic"), Some(&tokens(1000)));
    }

    #[test]
    fn forgetting_says_whether_there_was_anything_to_forget() {
        let mut store = Store::default();
        store.put("anthropic", tokens(1000));
        assert!(store.forget("anthropic"));
        assert!(!store.forget("anthropic"));
    }

    #[test]
    fn a_provider_that_issues_no_refresh_token_is_still_storable() {
        let json = r#"{"providers":{"p":{"access":"a","expires_at":1}}}"#;
        let store: Store = serde_json::from_str(json).expect("decode");
        assert_eq!(store.get("p").expect("tokens").refresh, None);
    }
}
