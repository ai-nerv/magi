//! What a surface may ask magi, and what comes back.
//!
//! Until now a tenant could draw and read the keyboard, and that was the whole of it. It was told
//! its rows, its columns and its call's arguments at open, and nothing after that — so a picker
//! could not list what this session remembers, and a game could not name the model it was being
//! played beside. Everything a surface knew, it had to be handed before it started.
//!
//! **The verb list is the gate.** Not a ledger entry: the ledger decides about reading a path,
//! writing one, running a command and reaching a host, and none of those is what this is. What a
//! surface may ask is instead a closed list of *facts about the session the person is already
//! looking at* — nothing here touches the filesystem, the network or a shell, and nothing here
//! can change anything. A verb magi does not know is refused by name rather than ignored, so a
//! tenant built against a newer magi is told so instead of waiting.
//!
//! **magi answers, the surface does not.** The same rule the rest of the surface protocol runs
//! on: a tenant asks, and what it gets back is what magi chose to say. It cannot reach past the
//! answer to the thing that produced it.

use serde::{Deserialize, Serialize};

/// One thing a surface may ask about.
///
/// Closed, and deliberately short. Every entry is a fact magi already holds about the session on
/// screen; the list grows when there is somewhere honest to get an answer from, which is why it
/// is an enum rather than a string a tenant makes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wonder {
    /// Which session this is: its id, the directory it runs in, and what it is called.
    Session,
    /// The model answering here, and how big its context window is.
    Model,
    /// What this session remembers, nearest first.
    ///
    /// Answered by balthasar, and refused where there is no balthasar — which is the ordinary
    /// case on a machine without it, and not an error.
    Memories,
}

/// Every verb, in the order a listing should show them.
pub const EVERY: &[Wonder] = &[Wonder::Session, Wonder::Model, Wonder::Memories];

impl Wonder {
    /// The verb, as it is written on the wire and in a refusal.
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Model => "model",
            Self::Memories => "memories",
        }
    }

    /// The verb by that name, or `None` where this magi has no such verb.
    ///
    /// The gate, and the reason a surface's question carries a *name* rather than a decoded verb:
    /// an unknown one has to survive as far as the answer, or a tenant built against a newer magi
    /// gets silence — and silence is indistinguishable from a magi still thinking about it.
    #[must_use]
    pub fn named(verb: &str) -> Option<Self> {
        EVERY.iter().copied().find(|known| known.verb() == verb)
    }
}

/// Which question an answer belongs to.
///
/// A surface may have more than one in flight — it asks, keeps drawing, and the answer arrives
/// some frames later — so an answer that did not say which question it was would be one a tenant
/// had to guess about.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Wondered(pub u64);

/// What magi says back.
///
/// A refusal is a *told* refusal. Silence would be indistinguishable from a magi still working it
/// out, and a tenant waiting on one that is never coming holds the rows until the surface times
/// out — which is the failure this whole layer was built to avoid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "answer")]
pub enum Answered {
    /// What was asked for.
    Told {
        /// The answer, shaped by the verb.
        said: serde_json::Value,
    },
    /// Nothing was said, and this is why.
    Refused {
        /// In words a tenant can put on the screen: the verb is not known here, or the sibling
        /// that would have answered it is not running.
        because: String,
    },
}

#[cfg(test)]
mod verbs {
    use super::*;

    #[test]
    fn a_verb_this_build_does_not_know_is_not_a_verb() {
        // The gate, in one assertion. A tenant cannot invent one — and the name survives being
        // refused, so what comes back says which question went unanswered.
        assert_eq!(Wonder::named("siblings"), None);
        assert_eq!(Wonder::named("memories"), Some(Wonder::Memories));
    }

    #[test]
    fn an_answer_is_written_the_way_casper_reads_one() {
        // **The one thing a test in this repository can get wrong for free.** casper is a separate
        // checkout with its own copy of these frames, so nothing here fails when the two spellings
        // drift — the surface simply stops being answered. Pinned against a literal for that
        // reason: this is the shape casper's own test reads back.
        let told = crate::surfacing::ToSurface::Answer {
            wondered: Wondered(3),
            answered: Answered::Told {
                said: serde_json::json!({ "id": "s-7" }),
            },
        };
        assert_eq!(
            serde_json::to_value(&told).expect("encodes"),
            serde_json::json!({
                "to": "answer",
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
                "to": "answer",
                "wondered": 4,
                "answer": "refused",
                "because": "memories: there is no balthasar in this session",
            })
        );
    }

    #[test]
    fn a_question_is_read_the_way_casper_writes_one() {
        // The other direction, and the same reason. This literal is what casper puts on the pipe.
        let asked: crate::surfacing::FromSurface =
            serde_json::from_str(r#"{"from":"ask","wondered":3,"wonder":"memories","args":{"query":"deploy","limit":3}}"#)
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
        // A refusal quotes the verb back, so the two spellings drifting apart would produce a
        // message naming something the tenant never sent.
        for verb in [Wonder::Session, Wonder::Model, Wonder::Memories] {
            let wire = serde_json::to_string(&verb).expect("encodes");
            assert_eq!(wire, format!("\"{}\"", verb.verb()));
        }
    }
}
