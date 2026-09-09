//! The contract for asking a mind a question.
//!
//! magi does not talk to models; melchior does, and this is the vocabulary magi asks it in.
//! Everything here is neutral about which model.
//!
//! Every type here is `Serialize + Deserialize` with no borrowed data and no untagged enums, so
//! the same value round-trips through JSON and through CBOR. JSON is what a person reads and what
//! a Lua sibling speaks — the family stub cannot decode CBOR. CBOR is what magi and melchior use
//! between themselves: it keeps byte strings and the integer/float split that JSON flattens, and
//! a signature is exactly the sort of opaque bytes JSON would mangle.
//!
//! An [`Ask`] answers with many [`Said`], not one. The family socket is request and reply, so the
//! stream travels on melchior's pipe instead: one JSON object per line, in order, ending in
//! [`Said::Stop`] or [`Said::Failed`]. A caller that sees neither has lost the mind.

use magi_model::{Context, StopReason, ThinkingLevel, Usage};
use serde::{Deserialize, Serialize};

/// One model, as melchior describes it. `api` is the interface it is spoken to over, which is
/// what keeps the rest of this neutral: magi never learns what an Anthropic request looks like.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// How an [`Ask`] names it: `provider/model`.
    pub id: String,
    pub provider: String,
    pub name: String,
    /// The wire protocol it is spoken to over — `anthropic`, `openai`, `google`. The interface,
    /// not the vendor: two providers reselling one model may speak different protocols.
    pub api: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    #[serde(default)]
    pub reasons: bool,
    #[serde(default)]
    pub ready: bool,
    /// What it would need to become ready: a variable name, never a value. Nothing here ever
    /// carries a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

/// What a caller wants beyond the conversation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Wants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingLevel>,
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
    pub schema: serde_json::Value,
}

/// One turn, handed over to be run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    /// Which model, by [`Card::id`].
    pub model: String,
    pub context: Context,
    #[serde(default)]
    pub wants: Wants,
    /// The caller's own name for this turn, quoted back on every [`Said`], so a broker driving
    /// more than one can tell the streams apart.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

/// Why a mind could not answer, carried across so a broker can act on it rather than only report
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Transport,
    Overload,
    Throttle,
    Auth,
    Invalid,
    /// The context window overflowed. Compact and ask again.
    Overflow,
    Unknown,
}

impl Refusal {
    #[must_use]
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Transport | Self::Overload | Self::Throttle)
    }
}

/// One thing that happened while an answer streamed. Smaller than any protocol's own event set:
/// what an adapter remembers between events stays on melchior's side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum Said {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    /// Opaque provider state for the block being streamed, replayed verbatim on the next request
    /// or the provider refuses it.
    Signature {
        signature: String,
    },
    ToolCallStart {
        /// Provider-issued identity, which the result must quote back.
        id: String,
        name: String,
    },
    /// Arguments for the tool call in progress, as raw JSON text.
    ToolCallArgs {
        args: String,
    },
    /// What the turn cost. Arrives at its own pace, and more than once.
    Spent {
        usage: Usage,
    },
    /// An attempt failed and another is starting. Everything said so far belongs to the
    /// attempt that failed and is retracted by this. Reported during the wait, not after it.
    Retrying {
        /// Which attempt just failed, counting from one.
        attempt: u32,
        of: u32,
        /// How long before the next one, in seconds.
        seconds: f64,
        why: String,
    },
    Stop {
        reason: StopReason,
    },
    /// The turn did not finish. Distinct from [`Said::Stop`] with an error reason: melchior
    /// could not run the turn at all.
    Failed {
        message: String,
        why: Refusal,
    },
}

impl Said {
    /// Whether this ends the stream. Anything after one belongs to another turn, and anything
    /// instead of one means the mind was lost rather than that it refused.
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
            assert!(text.contains("\"event\""), "untagged: {text}");
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
        assert!(text.contains("OPENROUTER_API_KEY"));
        assert!(!text.contains("sk-"), "a card must not carry a credential");
    }
}

/// The same shapes, through CBOR — the claim in this file's header is worth nothing unchecked.
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
        // Opaque provider state: a next request carrying anything but these exact bytes is refused.
        let said = Said::Signature {
            signature: "Er cB\u{1}\u{2}\u{7f} +/=".into(),
        };
        assert_eq!(through_cbor(&said), said);
        let text = serde_json::to_string(&said).expect("json");
        assert_eq!(serde_json::from_str::<Said>(&text).expect("decode"), said);
    }
}
