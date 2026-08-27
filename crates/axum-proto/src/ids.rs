//! Identity newtypes.
//!
//! Distinct types rather than a shared `String` alias: a `ToolCallId` must never be accepted
//! where a `MessageId` belongs, and the compiler is the cheapest place to enforce that.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Wrap a string as a `", stringify!($name), "`.")]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Borrow the underlying string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id! {
    /// Identifies a session, and names its journal file.
    SessionId
}

string_id! {
    /// Identifies one message within a session.
    MessageId
}

string_id! {
    /// Identifies one tool call, as the provider issued it.
    ToolCallId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_display() {
        assert_eq!(SessionId::new("s-1").to_string(), "s-1");
        assert_eq!(MessageId::new("m-1").as_str(), "m-1");
        assert_eq!(ToolCallId::new("t-1").as_str(), "t-1");
    }
}
