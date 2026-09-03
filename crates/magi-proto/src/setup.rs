//! Telling a sibling how to behave.
//!
//! magi coordinates. A sibling it started should not be reading a configuration file of its own
//! and hoping the two agree — magi decides, and says so. So a sibling exposes two verbs beside
//! its own vocabulary:
//!
//! | verb | |
//! |---|---|
//! | `needs` | what it wants to be told, as [`Need`]s — the questions, not the answers |
//! | `configure` | a chunk of its config Lua, run in its own VM, answering [`Applied`] |
//!
//! **Both directions matter.** A coordinator that only pushed settings would have to know every
//! sibling's vocabulary by heart, and would be wrong first — so the sibling declares what it
//! takes, and the coordinator answers what it knows. A setting nobody asked for is refused
//! rather than silently ignored, because a typo that does nothing is the worst kind.
//!
//! # Why Lua and not a table of values
//!
//! Because the config API *is* Lua, and a data-only second surface would be a second thing to
//! keep in step — the failure this whole arrangement exists to avoid. A sibling runs what it is
//! sent through the same sandboxed VM, the same registrars and the same refusals as the file it
//! would have read. Nothing new is trusted.
//!
//! # What is trusted
//!
//! The peer, by the kernel. A sibling accepts configuration only from the same uid — the family
//! already refuses anything else at the socket — and that uid can already edit the config file
//! this replaces, pass argv, and set the environment. The VM is the sandboxed one: no
//! `os.execute`, no `io`, no filesystem. This is not a new privilege; it is the existing one
//! arriving down a pipe instead of off a disk.
//!
//! A sibling started *without* a coordinator reads its own files exactly as before. Configuring
//! over the wire is what happens when somebody is coordinating, not instead of.

use serde::{Deserialize, Serialize};

/// What kind of value a setting takes.
///
/// Deliberately coarse. This says enough for a coordinator to send the right shape and for a
/// person to read the list; anything finer would be a schema language, and the sibling is going
/// to validate what it receives regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A string.
    Text,
    /// A number.
    Number,
    /// True or false.
    Flag,
    /// A table — a list or a map, and the sibling says which in `about`.
    Table,
}

/// One thing a sibling wants to be told.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Need {
    /// What to set, as the config names it: `thinking`, `retention.days`.
    pub name: String,
    /// What sort of value it takes.
    pub kind: Kind,
    /// One line, for a person reading the list.
    pub about: String,
    /// Whether the sibling cannot work without it.
    ///
    /// Most settings are not: a sibling with a sensible default should say so rather than
    /// demand an answer, and a coordinator that had to fill in twenty fields to start one would
    /// be a coordinator nobody uses.
    #[serde(default)]
    pub required: bool,
    /// What it does when nothing is said, when that is expressible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// What a `configure` call did.
///
/// Named rather than counted: "3 settings applied" cannot be checked against what was sent, and
/// the case that matters is the one where a coordinator sent something the sibling does not
/// take. That is a refusal with a reason, never silence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// Settings that took effect, by name.
    #[serde(default)]
    pub set: Vec<String>,
    /// Settings that were sent and not taken, with why.
    #[serde(default)]
    pub refused: Vec<Refused>,
}

impl Applied {
    /// Whether everything sent was taken.
    #[must_use]
    pub fn whole(&self) -> bool {
        self.refused.is_empty()
    }
}

/// One setting a sibling would not take.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refused {
    /// What was sent.
    pub name: String,
    /// Why it was not taken.
    pub why: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need() -> Need {
        Need {
            name: "thinking".into(),
            kind: Kind::Text,
            about: "how much reasoning to ask a model for".into(),
            required: false,
            default: Some(serde_json::json!("off")),
        }
    }

    #[test]
    fn a_need_survives_json() {
        let text = serde_json::to_string(&need()).expect("encode");
        assert_eq!(serde_json::from_str::<Need>(&text).expect("decode"), need());
    }

    #[test]
    fn a_need_with_no_default_omits_it_rather_than_saying_null() {
        let bare = Need {
            default: None,
            ..need()
        };
        let text = serde_json::to_string(&bare).expect("encode");
        assert!(!text.contains("default"), "{text}");
    }

    #[test]
    fn an_empty_answer_is_a_whole_one() {
        assert!(Applied::default().whole());
        let partial = Applied {
            set: vec!["thinking".into()],
            refused: vec![Refused {
                name: "colour".into(),
                why: "not a setting this takes".into(),
            }],
        };
        assert!(!partial.whole(), "a refusal is not silence");
    }

    #[test]
    fn every_kind_round_trips_by_name() {
        for kind in [Kind::Text, Kind::Number, Kind::Flag, Kind::Table] {
            let text = serde_json::to_string(&kind).expect("encode");
            assert!(text.starts_with('"'), "named, not numbered: {text}");
            assert_eq!(serde_json::from_str::<Kind>(&text).expect("decode"), kind);
        }
    }

    #[test]
    fn the_whole_exchange_survives_cbor_as_well() {
        // Both encodings, like everything else that crosses between siblings.
        let applied = Applied {
            set: vec!["thinking".into()],
            refused: Vec::new(),
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&applied, &mut bytes).expect("encode");
        let back: Applied = ciborium::from_reader(bytes.as_slice()).expect("decode");
        assert_eq!(back, applied);
    }
}
