use std::{cell::Cell, fmt};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

struct Unique(Value);

struct Budget<'a> {
    remaining: &'a Cell<usize>,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for Budget<'_> {
    type Value = Unique;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Unique, D::Error> {
        let Some(left) = self.remaining.get().checked_sub(1) else {
            return Err(de::Error::custom("ACP JSON node limit"));
        };
        if self.depth > 64 {
            return Err(de::Error::custom("ACP JSON depth limit"));
        }
        self.remaining.set(left);
        deserializer.deserialize_any(UniqueVisitor(self))
    }
}

struct UniqueVisitor<'a>(Budget<'a>);

impl<'de> Visitor<'de> for UniqueVisitor<'_> {
    type Value = Unique;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object fields")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom("duplicate object field"));
            }
            object.insert(
                key,
                map.next_value_seed(Budget {
                    remaining: self.0.remaining,
                    depth: self.0.depth + 1,
                })?
                .0,
            );
        }
        Ok(Unique(Value::Object(object)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut array: A) -> Result<Unique, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = array.next_element_seed(Budget {
            remaining: self.0.remaining,
            depth: self.0.depth + 1,
        })? {
            values.push(value.0);
        }
        Ok(Unique(Value::Array(values)))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Unique, E> {
        Ok(Unique(Value::String(value.to_owned())))
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Unique, E> {
        Ok(Unique(Value::Bool(value)))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Unique, E> {
        Ok(Unique(Value::Number(value.into())))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Unique, E> {
        Ok(Unique(Value::Number(value.into())))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Unique, E> {
        Number::from_f64(value)
            .map(|n| Unique(Value::Number(n)))
            .ok_or_else(|| de::Error::custom("nonfinite number"))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
}

pub(super) fn parse(bytes: &[u8]) -> serde_json::Result<Value> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let remaining = Cell::new(65_536);
    let value = Budget {
        remaining: &remaining,
        depth: 0,
    }
    .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value.0)
}
