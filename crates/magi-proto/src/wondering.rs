//! What a surface may ask magi, and what comes back.
//!
//! A closed list of facts about the session on screen: nothing here touches the filesystem, the
//! network or a shell, and nothing here can change anything. A verb magi does not know is refused
//! by name rather than ignored, so a tenant built against a newer magi is told so instead of
//! waiting.

use serde::{Deserialize, Serialize};

/// One thing a surface may ask about. An enum rather than a name a tenant makes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wonder {
    /// Which session this is: its id, the directory it runs in, and what it is called.
    Session,
    /// The model answering here, and how big its context window is.
    Model,
    /// What this session remembers, nearest first. Answered by balthasar, and refused where there
    /// is no balthasar, which is not an error.
    Memories,
}

pub const EVERY: &[Wonder] = &[Wonder::Session, Wonder::Model, Wonder::Memories];

impl Wonder {
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Model => "model",
            Self::Memories => "memories",
        }
    }

    /// The verb by that name, or `None` where this magi has no such verb. An unknown name has to
    /// survive as far as the answer, or a tenant built against a newer magi gets silence.
    #[must_use]
    pub fn named(verb: &str) -> Option<Self> {
        EVERY.iter().copied().find(|known| known.verb() == verb)
    }
}

/// Which question an answer belongs to. A surface may have more than one in flight.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Wondered(pub u64);

/// What magi says back. A refusal is a told refusal: silence is indistinguishable from a magi
/// still working it out, and holds the rows until the surface times out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "answer")]
pub enum Answered {
    Told {
        said: serde_json::Value,
    },
    Refused {
        /// In words a tenant can put on the screen.
        because: String,
    },
}

#[cfg(test)]
mod verbs {
    use super::*;

    #[test]
    fn a_verb_this_build_does_not_know_is_not_a_verb() {
        assert_eq!(Wonder::named("siblings"), None);
        assert_eq!(Wonder::named("memories"), Some(Wonder::Memories));
    }

    #[test]
    fn an_answer_is_written_the_way_casper_reads_one() {
        // casper is a separate checkout with its own copy of these frames, so nothing here
        // fails when the two spellings drift. Pinned against the literal casper reads back.
        let told = crate::surfacing::ToSurface::Answer {
            wondered: Wondered(3),
            answered: Answered::Told {
                said: serde_json::json!({ "id": "s-7" }),
            },
        };
        assert_eq!(
            serde_json::to_value(&told).expect("encodes"),
            serde_json::json!({
                "event": "answer",
                "wondered": 3,
                "answer": "told",
                "said": { "id": "s-7" },
            })
        );

        let refused = crate::surfacing::ToSurface::Answer {
            wondered: Wondered(4),
            answered: Answered::Refused {
                because: "memories: there is no balthasar in this session".to_owned(),
            },
        };
        assert_eq!(
            serde_json::to_value(&refused).expect("encodes"),
            serde_json::json!({
                "event": "answer",
                "wondered": 4,
                "answer": "refused",
                "because": "memories: there is no balthasar in this session",
            })
        );
    }

    #[test]
    fn a_question_is_read_the_way_casper_writes_one() {
        let asked: crate::surfacing::FromSurface =
            serde_json::from_str(r#"{"event":"ask","wondered":3,"wonder":"memories","args":{"query":"deploy","limit":3}}"#)
                .expect("decodes");
        let crate::surfacing::FromSurface::Ask {
            wondered,
            wonder,
            args,
        } = asked
        else {
            panic!("a surface asked something: {asked:?}");
        };
        assert_eq!(wondered, Wondered(3));
        assert_eq!(Wonder::named(&wonder), Some(Wonder::Memories));
        assert_eq!(args["query"], "deploy");
    }

    #[test]
    fn a_refusal_says_why_rather_than_saying_nothing() {
        let refused = Answered::Refused {
            because: "there is no balthasar here".to_owned(),
        };
        let wire = serde_json::to_string(&refused).expect("encodes");
        assert!(wire.contains("balthasar"), "{wire}");
        assert_eq!(
            serde_json::from_str::<Answered>(&wire).expect("decodes"),
            refused
        );
    }

    #[test]
    fn every_verb_is_named_the_same_on_the_wire_and_in_a_refusal() {
        for verb in [Wonder::Session, Wonder::Model, Wonder::Memories] {
            let wire = serde_json::to_string(&verb).expect("encodes");
            assert_eq!(wire, format!("\"{}\"", verb.verb()));
        }
    }
}
