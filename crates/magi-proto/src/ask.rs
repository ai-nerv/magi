//! The contract for asking a mind a question.
//!
//! magi does not talk to models. melchior does: it owns the protocols, the credentials and the
//! HTTP, and this is the vocabulary magi asks it in. Everything here is deliberately neutral
//! about *which* model — a card describes one, an [`Ask`] names one, and neither knows what a
//! provider is.
//!
//! # Two encodings, one shape
//!
//! Every type here is `Serialize + Deserialize` with no borrowed data and no untagged enums, so
//! the same value round-trips through JSON and through CBOR. JSON is what a person reads and
//! what a Lua sibling speaks — the family stub cannot decode CBOR. CBOR is what magi and
//! melchior use between themselves once neither needs to read it: it keeps byte strings and the
//! integer/float split that JSON flattens, and a signature is exactly the sort of opaque bytes
//! JSON would quietly mangle.
//!
//! Both are offered because neither is right everywhere, and a family whose members guess is a
//! family that fails on the first signature.
//!
//! # Streaming
//!
//! An [`Ask`] answers with many [`Said`], not one. The family socket is request and reply, so
//! the stream travels on melchior's pipe instead: one JSON object per line, in order, ending in
//! [`Said::Stop`] or [`Said::Failed`]. A caller that sees neither has lost the mind, which is
//! the only failure it must tell apart from a refusal.

use magi_model::{Context, StopReason, ThinkingLevel, Usage};
use serde::{Deserialize, Serialize};

/// One model, as melchior describes it.
///
/// What a picker shows and what an [`Ask`] names. `api` is the *interface* — which wire protocol
/// this model is spoken to over — and it is the field that makes the rest of it neutral: magi
/// never learns what an Anthropic request looks like, only that this model wants `anthropic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// How an [`Ask`] names it: `provider/model`.
    pub id: String,
    /// Who serves it.
    pub provider: String,
    /// What the provider calls it.
    pub name: String,
    /// The wire protocol it is spoken to over — `anthropic`, `openai`, `google`.
    ///
    /// The interface, not the vendor. Two providers reselling one model may speak different
    /// protocols, and one provider may speak several.
    pub api: String,
    /// How much it will read, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// How much it will write, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    /// Whether it reasons, so a caller knows whether asking for thinking means anything.
    #[serde(default)]
    pub reasons: bool,
    /// Whether melchior could talk to it right now.
    #[serde(default)]
    pub ready: bool,
    /// What it would need to become ready, when it is not.
    ///
    /// A variable name, not a value. Nothing here ever carries a credential: melchior resolves
    /// those and magi has no reason to see one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

/// What a caller wants beyond the conversation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Wants {
    /// How much reasoning to ask for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingLevel>,
    /// Cap the answer below the model's own maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// A JSON Schema the answer must satisfy, and what to call it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
}

/// A named JSON Schema an answer must satisfy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    /// What to call it. Some providers require a name and none of them show it to anybody.
    pub name: String,
    /// The schema itself.
    pub schema: serde_json::Value,
}

/// One turn, handed over to be run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    /// Which model, by [`Card::id`].
    pub model: String,
    /// The conversation, the system prompt, and the tools that may be called.
    pub context: Context,
    /// Everything else the caller wants.
    #[serde(default)]
    pub wants: Wants,
    /// The caller's own name for this turn, quoted back on every [`Said`].
    ///
    /// So a broker driving more than one turn can tell the streams apart without a connection
    /// each.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

/// Why a mind could not answer.
///
/// The same seven a provider's own classifier produces, carried across so a broker can act on
/// them rather than only report them. `Overflow` is the one that earns its place on its own:
/// it is answered by compacting the conversation and asking again, and a broker told only
/// "it failed, try later" would give up on a turn that a summary would have fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The connection failed before an answer arrived.
    Transport,
    /// The mind is busy. Worth waiting.
    Overload,
    /// Rate limited. Worth waiting longer.
    Throttle,
    /// Credentials are missing or rejected. Nothing to wait for.
    Auth,
    /// The request was malformed, or asked for something unavailable.
    Invalid,
    /// The context window overflowed. Compact and ask again.
    Overflow,
    /// Unrecognised.
    Unknown,
}

impl Refusal {
    /// Whether asking again, unchanged, could work.
    #[must_use]
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Transport | Self::Overload | Self::Throttle)
    }
}

/// One thing that happened while an answer streamed.
///
/// Smaller than any protocol's own event set on purpose: a broker wants to know that text
/// arrived, not that a content block of index three opened. What an adapter remembers between
/// events stays on melchior's side, where the adapter is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "said")]
pub enum Said {
    /// Answer text.
    Text {
        /// The text that arrived.
        text: String,
    },
    /// Reasoning text, which is shown differently and sent back differently.
    Thinking {
        /// The reasoning that arrived.
        text: String,
    },
    /// Opaque provider state for the block being streamed.
    ///
    /// Replayed verbatim on the next request or the provider refuses it. Carried as a string
    /// here because that is what every provider issues; CBOR keeps it byte-exact and JSON keeps
    /// it because it is already text.
    Signature {
        /// The token standing for reasoning already done.
        signature: String,
    },
    /// A tool call began.
    ToolCallStart {
        /// Provider-issued identity, which the result must quote back.
        id: String,
        /// Which tool.
        name: String,
    },
    /// Arguments for the tool call in progress, as raw JSON text.
    ToolCallArgs {
        /// The fragment that arrived.
        args: String,
    },
    /// What the turn cost. Arrives at its own pace, and more than once.
    Spent {
        /// Tokens, by kind.
        usage: Usage,
    },
    /// An attempt failed and another is starting.
    ///
    /// Everything said so far belongs to the attempt that failed and is retracted by this: half
    /// an answer concatenated with a whole one parses as neither. Reported *during* the wait
    /// rather than after it, because the whole value of saying "retrying" is saying it while
    /// somebody is watching a spinner and wondering whether it has hung.
    Retrying {
        /// Which attempt just failed, counting from one.
        attempt: u32,
        /// How many will be made in all.
        of: u32,
        /// How long before the next one, in seconds.
        seconds: f64,
        /// What went wrong, for the person watching.
        why: String,
    },
    /// The turn finished, and why.
    Stop {
        /// What ended it.
        reason: StopReason,
    },
    /// The turn did not finish.
    ///
    /// Distinct from [`Said::Stop`] with an error reason: this is melchior saying it could not
    /// run the turn at all, which is a different thing from a model that answered badly.
    Failed {
        /// What went wrong, in a sentence.
        message: String,
        /// Which kind of failure, so a broker can act rather than only report.
        why: Refusal,
    },
}

impl Said {
    /// Whether this ends the stream.
    ///
    /// A caller reads until one of these. Anything after it belongs to another turn, and
    /// anything instead of it means the mind was lost rather than that it refused.
    #[must_use]
    pub fn is_last(&self) -> bool {
        matches!(self, Said::Stop { .. } | Said::Failed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::{Content, Message, Role};

    pub(super) fn an_ask() -> Ask {
        Ask {
            model: "openrouter/anthropic/claude-sonnet-4.5".into(),
            context: Context {
                system: Some("be brief".into()),
                messages: vec![Message {
                    role: Role::User,
                    content: vec![Content::Text {
                        text: "hello".into(),
                        signature: None,
                    }],
                    stop_reason: None,
                    usage: None,
                    error: None,
                }],
                tools: Vec::new(),
            },
            wants: Wants {
                thinking: Some(ThinkingLevel::Medium),
                max_tokens: Some(256),
                schema: None,
            },
            about: "t1".into(),
        }
    }

    #[test]
    fn an_ask_survives_json() {
        let text = serde_json::to_string(&an_ask()).expect("encode");
        assert_eq!(
            serde_json::from_str::<Ask>(&text).expect("decode"),
            an_ask()
        );
    }

    #[test]
    fn every_said_survives_json_and_says_which_it_is() {
        let stream = [
            Said::Text { text: "hi".into() },
            Said::Thinking {
                text: "mulling".into(),
            },
            Said::Signature {
                signature: "opaque".into(),
            },
            Said::ToolCallStart {
                id: "t1".into(),
                name: "shell".into(),
            },
            Said::ToolCallArgs {
                args: "{\"command\":".into(),
            },
            Said::Spent {
                usage: Usage::default(),
            },
            Said::Stop {
                reason: StopReason::EndTurn,
            },
            Said::Failed {
                message: "no credential".into(),
                why: Refusal::Auth,
            },
        ];
        for said in stream {
            let text = serde_json::to_string(&said).expect("encode");
            assert!(text.contains("\"said\""), "untagged: {text}");
            assert_eq!(serde_json::from_str::<Said>(&text).expect("decode"), said);
        }
    }

    #[test]
    fn only_the_last_two_end_a_stream() {
        assert!(
            Said::Stop {
                reason: StopReason::EndTurn
            }
            .is_last()
        );
        assert!(
            Said::Failed {
                message: String::new(),
                why: Refusal::Transport
            }
            .is_last()
        );
        assert!(
            !Said::Text {
                text: "more coming".into()
            }
            .is_last()
        );
    }

    #[test]
    fn a_card_says_which_interface_without_naming_a_credential() {
        let card = Card {
            id: "openrouter/x".into(),
            provider: "openrouter".into(),
            name: "x".into(),
            api: "openai".into(),
            context_window: Some(200_000),
            max_output: None,
            reasons: true,
            ready: false,
            needs: Some("OPENROUTER_API_KEY".into()),
        };
        let text = serde_json::to_string(&card).expect("encode");
        assert_eq!(serde_json::from_str::<Card>(&text).expect("decode"), card);
        // The variable is named; its value is melchior's and never travels.
        assert!(text.contains("OPENROUTER_API_KEY"));
        assert!(!text.contains("sk-"), "a card must not carry a credential");
    }
}

/// The same shapes, through CBOR.
///
/// Its own module because the claim in this file's header — one shape, two encodings — is worth
/// nothing unless something checks it. A field that JSON tolerates and CBOR does not, or an
/// enum representation that only survives one of them, would otherwise be found by whichever
/// sibling happened to pick the other.
#[cfg(test)]
mod cbor_tests {
    use super::tests::an_ask;
    use super::*;

    fn through_cbor<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes).expect("encode");
        ciborium::from_reader(bytes.as_slice()).expect("decode")
    }

    #[test]
    fn an_ask_survives_cbor_as_well_as_json() {
        assert_eq!(through_cbor(&an_ask()), an_ask());
    }

    #[test]
    fn a_tagged_said_survives_cbor() {
        let said = Said::ToolCallStart {
            id: "t1".into(),
            name: "shell".into(),
        };
        assert_eq!(through_cbor(&said), said);
    }

    #[test]
    fn a_signature_comes_back_byte_for_byte() {
        // The reason CBOR is offered at all: this is opaque provider state, and a next request
        // carrying anything but these exact bytes is refused.
        let said = Said::Signature {
            signature: "Er cB\u{1}\u{2}\u{7f} +/=".into(),
        };
        assert_eq!(through_cbor(&said), said);
        let text = serde_json::to_string(&said).expect("json");
        assert_eq!(serde_json::from_str::<Said>(&text).expect("decode"), said);
    }
}
