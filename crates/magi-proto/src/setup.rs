//! Telling a sibling how to behave.
//!
//! magi decides configuration and says so, rather than a sibling reading a file of its own and
//! hoping the two agree. A sibling exposes two verbs beside its own vocabulary: `needs`, which
//! declares what it takes as [`Need`]s, and `configure`, which runs a chunk of config Lua in its
//! own sandboxed VM and answers [`Applied`]. A setting nobody asked for is refused with a reason
//! rather than ignored. Configuration is accepted only from the same uid, which is the uid that
//! could already edit the file this replaces. A sibling started without a coordinator reads its
//! own files exactly as before.

use serde::{Deserialize, Serialize};

/// What kind of value a setting takes. Coarse: the sibling validates what it receives regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Number,
    Flag,
    /// A table — a list or a map, and the sibling says which in `about`.
    Table,
}

/// One thing a sibling wants to be told.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Need {
    /// What to set, as the config names it: `thinking`, `retention.days`.
    pub name: String,
    pub kind: Kind,
    pub about: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// What a `configure` call did, named rather than counted, so a refusal names the setting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    #[serde(default)]
    pub set: Vec<String>,
    #[serde(default)]
    pub refused: Vec<Refused>,
}

impl Applied {
    #[must_use]
    pub fn whole(&self) -> bool {
        self.refused.is_empty()
    }
}

/// One setting a sibling would not take.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refused {
    pub name: String,
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
