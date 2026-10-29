//! Model-neutral requests from Magi to Melchior, with owned JSON/CBOR-serializable types.
//! An [`Ask`] produces ordered [`Said`] events on Melchior's newline-delimited JSON pipe.
//! The stream must end with [`Said::Stop`] or [`Said::Failed`].

use magi_model::{Context, StopReason, ThinkingLevel, Usage};
use serde::{Deserialize, Serialize};

pub use magi_model::overflow;

/// Published USD per million tokens, with absent rates left unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pricing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// One model, as melchior describes it. `api` is the interface it is spoken to over, which is
/// what keeps the rest of this neutral: magi never learns what an Anthropic request looks like.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub vision: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<Pricing>,
    /// Position in OpenRouter's weekly popularity-sorted model catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popularity_rank: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<magi_model::Capabilities>,
    #[serde(default)]
    pub reasons: bool,
    #[serde(default)]
    pub ready: bool,
    /// What it would need to become ready: a variable name, never a value. Nothing here ever
    /// carries a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

/// What a caller wants beyond the conversation: reasoning, a cap, a schema, and which provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Wants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub max_tokens_automatic: bool,
    /// A JSON Schema the answer must satisfy, and what to call it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
    /// An input or output constraint failed; optional detail distinguishes its cause.
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
    RequestFrame {
        frame: magi_model::RequestFrame,
    },
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
    /// Cumulative usage of the whole ask, failed attempts included; replaces the last measurement.
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
    /// Which of a router's upstreams answered: asking it first next time finds the prompt cached.
    Served {
        provider: String,
    },
    Stop {
        reason: StopReason,
    },
    /// The turn did not finish. Distinct from [`Said::Stop`] with an error reason: melchior
    /// could not run the turn at all.
    Failed {
        message: String,
        why: Refusal,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "overflow::optional"
        )]
        overflow: Option<overflow::Detail>,
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
mod accounting_compatibility {
    use super::*;
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Legacy {
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost_micros: u64,
    }

    #[test]
    fn reported_and_estimated_costs_cross_json_and_cbor_without_changing_old_fields() {
        let legacy = Legacy {
            input: 10,
            output: 7,
            cache_read: 90,
            cache_write: 0,
            cost_micros: 4,
        };
        let raw = serde_json::to_value(&legacy).expect("legacy json");
        let mut new: Usage = serde_json::from_value(raw.clone()).expect("new reader");
        assert_eq!(new.estimated_micros, None);
        assert_eq!(serde_json::to_value(new).expect("legacy shape"), raw);
        new.estimated_micros = Some(3);
        assert_eq!(new.charged_micros(), 7);
        assert_eq!(
            serde_json::from_value::<Legacy>(serde_json::to_value(new).expect("new json"))
                .expect("old reader"),
            legacy
        );
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut encoded).expect("old cbor");
        let read: Usage = ciborium::de::from_reader(encoded.as_slice()).expect("new reader");
        assert_eq!(read.estimated_micros, None);
        encoded.clear();
        ciborium::ser::into_writer(&new, &mut encoded).expect("new cbor");
        assert_eq!(
            ciborium::de::from_reader::<Legacy, _>(encoded.as_slice()).expect("old reader"),
            legacy
        );
        let event = Said::Spent { usage: new };
        encoded.clear();
        ciborium::ser::into_writer(&event, &mut encoded).expect("event cbor");
        assert_eq!(
            ciborium::de::from_reader::<Said, _>(encoded.as_slice()).expect("event"),
            event
        );
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
                max_tokens_automatic: false,
                schema: None,
                provider: None,
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
    fn output_provenance_is_additive_and_legacy_limits_remain_explicit() {
        let old: Wants =
            serde_json::from_value(serde_json::json!({"max_tokens":77})).expect("old ask");
        assert!(!old.max_tokens_automatic);
        assert_eq!(
            serde_json::to_value(&old).expect("legacy"),
            serde_json::json!({"max_tokens":77})
        );
        let automatic = Wants {
            max_tokens_automatic: true,
            ..old
        };
        let value = serde_json::to_value(&automatic).expect("automatic");
        assert_eq!(
            serde_json::from_value::<Wants>(value).expect("JSON"),
            automatic
        );
        let mut bytes = Vec::new();
        ciborium::into_writer(&automatic, &mut bytes).expect("encode CBOR");
        assert_eq!(
            ciborium::from_reader::<Wants, _>(bytes.as_slice()).expect("CBOR"),
            automatic
        );
    }

    #[test]
    fn every_said_survives_json_and_says_which_it_is() {
        let stream = [
            Said::RequestFrame {
                frame: magi_model::RequestFrame {
                    version: 1,
                    dialect: [1; 32],
                    policy: [2; 32],
                    omitted_images: 3,
                },
            },
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
            Said::Served {
                provider: "StreamLake".into(),
            },
            Said::Stop {
                reason: StopReason::EndTurn,
            },
            Said::Failed {
                message: "no credential".into(),
                why: Refusal::Auth,
                overflow: None,
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
                why: Refusal::Transport,
                overflow: None,
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
            pricing: None,
            popularity_rank: None,
            capabilities: None,
            vision: false,
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

    #[test]
    fn model_metadata_survives_json_and_cbor_with_old_cards_still_readable() {
        let value = serde_json::json!({"id":"openrouter/vendor/model","provider":"openrouter","name":"model",
            "api":"openai-completions","pricing":{"input":"0","output":"0.3"},"popularity_rank":7,
            "capabilities":{"input":["text","image"],"output":["text"],"thinking":["low","high"]}});
        let card: Card = serde_json::from_value(value).expect("card");
        assert_eq!(through_cbor(&card), card);
        let text = serde_json::to_string(&card).expect("JSON");
        assert_eq!(serde_json::from_str::<Card>(&text).expect("JSON"), card);
        let old: Card = serde_json::from_value(serde_json::json!({"id":"local/model","provider":"local","name":"model","api":"openai-completions"})).expect("old card");
        assert!(old.pricing.is_none());
        assert!(old.popularity_rank.is_none());
        assert!(old.capabilities.is_none());
    }

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
    fn provider_frame_metadata_survives_cbor_with_bounded_fingerprints() {
        let said = Said::RequestFrame {
            frame: magi_model::RequestFrame {
                version: 1,
                dialect: [7; 32],
                policy: [11; 32],
                omitted_images: u64::MAX,
            },
        };
        assert_eq!(through_cbor(&said), said);
        let value = serde_json::to_value(&said).expect("json");
        assert_eq!(value["event"], "request_frame");
        assert_eq!(value["frame"]["omitted_images"], u64::MAX);
        for length in [0, 31, 33] {
            let mut invalid = value.clone();
            invalid["frame"]["dialect"] = serde_json::json!(vec![0; length]);
            assert!(serde_json::from_value::<Said>(invalid).is_err());
        }
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
