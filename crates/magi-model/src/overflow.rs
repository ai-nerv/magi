use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Input,
    OutputBudget,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detail {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_output_tokens: Option<u64>,
}

enum Atom {
    Word(&'static str),
    Count(u64),
    Null,
    Invalid,
}

struct AtomVisitor;

impl<'de> Visitor<'de> for AtomVisitor {
    type Value = Atom;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("optional overflow metadata")
    }
    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Atom, E> {
        Ok(Atom::Word(match text {
            "kind" => "kind",
            "prompt_tokens" => "prompt_tokens",
            "limit_tokens" => "limit_tokens",
            "requested_output_tokens" => "requested_output_tokens",
            "input" => "input",
            "output_budget" => "output_budget",
            _ => "",
        }))
    }
    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Atom, E> {
        Ok(Atom::Count(n))
    }
    fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Atom, E> {
        Ok(u64::try_from(n).map_or(Atom::Invalid, Atom::Count))
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Atom, E> {
        Ok(Atom::Invalid)
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Atom, E> {
        Ok(Atom::Invalid)
    }
    fn visit_bytes<E: serde::de::Error>(self, _: &[u8]) -> Result<Atom, E> {
        Ok(Atom::Invalid)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Atom, E> {
        Ok(Atom::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Atom, E> {
        Ok(Atom::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> Result<Atom, A::Error> {
        while values.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Atom::Invalid)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut values: A) -> Result<Atom, A::Error> {
        while values.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Atom::Invalid)
    }
}

impl<'de> Deserialize<'de> for Atom {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_any(AtomVisitor)
    }
}

struct DetailVisitor;

impl<'de> Visitor<'de> for DetailVisitor {
    type Value = Option<Detail>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("optional overflow detail")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Self::Value, A::Error> {
        let mut detail = Detail {
            kind: Kind::Unknown,
            prompt_tokens: None,
            limit_tokens: None,
            requested_output_tokens: None,
        };
        let (mut seen, mut invalid) = (0_u8, false);
        while let Some(key) = fields.next_key::<Atom>()? {
            let bit = match key {
                Atom::Word("kind") => 1,
                Atom::Word("prompt_tokens") => 2,
                Atom::Word("limit_tokens") => 4,
                Atom::Word("requested_output_tokens") => 8,
                Atom::Word(_) => {
                    fields.next_value::<IgnoredAny>()?;
                    continue;
                }
                _ => {
                    fields.next_value::<IgnoredAny>()?;
                    invalid = true;
                    continue;
                }
            };
            invalid |= seen & bit != 0;
            seen |= bit;
            let value = fields.next_value::<Atom>()?;
            if bit == 1 {
                detail.kind = match value {
                    Atom::Word("input") => Kind::Input,
                    Atom::Word("output_budget") => Kind::OutputBudget,
                    Atom::Word(_) => Kind::Unknown,
                    _ => {
                        invalid = true;
                        Kind::Unknown
                    }
                };
            } else {
                let count = match value {
                    Atom::Count(n) => Some(n),
                    Atom::Null => None,
                    _ => {
                        invalid = true;
                        None
                    }
                };
                match bit {
                    2 => detail.prompt_tokens = count,
                    4 => detail.limit_tokens = count,
                    _ => detail.requested_output_tokens = count,
                }
            }
        }
        Ok((!invalid && seen & 1 != 0).then_some(detail))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> Result<Self::Value, A::Error> {
        while values.next_element::<IgnoredAny>()?.is_some() {}
        Ok(None)
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_bytes<E: serde::de::Error>(self, _: &[u8]) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}

/// Decode optional provider overflow metadata.
///
/// # Errors
/// Returns the format error when the enclosing data cannot be decoded.
pub fn optional<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<Detail>, D::Error> {
    decoder.deserialize_any(DetailVisitor)
}
