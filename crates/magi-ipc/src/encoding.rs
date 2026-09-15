//! Which encoding a body on the family wire is in. Nothing is negotiated: a body says what it is
//! in its first byte, so a reply can be read without having been told what to expect.

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Wire {
    #[default]
    Json,
    Cbor,
}

impl Wire {
    /// Which encoding `body` is in: JSON's top level begins `{` or `[` after any leading space,
    /// CBOR's first byte is major type 4 or 5 — `0x80`–`0xBF`. The ranges do not overlap.
    #[must_use]
    pub fn of(body: &[u8]) -> Self {
        match body.iter().find(|b| !b.is_ascii_whitespace()) {
            Some(0x80..=0xBF) => Self::Cbor,
            _ => Self::Json,
        }
    }

    /// Read `body` as `T`, in whichever encoding it turns out to be.
    ///
    /// # Errors
    /// When the body is not that shape.
    pub fn read<T: DeserializeOwned>(body: &[u8]) -> Result<T, String> {
        match Self::of(body) {
            Self::Json => serde_json::from_slice(body).map_err(|why| why.to_string()),
            Self::Cbor => ciborium::from_reader(body).map_err(|why| why.to_string()),
        }
    }

    /// Write `value` in this encoding.
    ///
    /// # Errors
    /// When the value will not encode, which for the shapes on this wire cannot happen.
    pub fn write<T: Serialize>(self, value: &T) -> Result<Vec<u8>, String> {
        match self {
            Self::Json => serde_json::to_vec(value).map_err(|why| why.to_string()),
            Self::Cbor => {
                let mut bytes = Vec::new();
                ciborium::into_writer(value, &mut bytes).map_err(|why| why.to_string())?;
                Ok(bytes)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_says_which_encoding_it_is() {
        assert_eq!(Wire::of(br#"{"ok":true}"#), Wire::Json);
        assert_eq!(Wire::of(b"  \n[1]"), Wire::Json, "after space");
        let cbor = Wire::Cbor
            .write(&serde_json::json!({"ok": true}))
            .expect("encode");
        assert_eq!(Wire::of(&cbor), Wire::Cbor);
    }

    #[test]
    fn a_reply_is_read_without_being_told_which_it_is() {
        for wire in [Wire::Json, Wire::Cbor] {
            let body = wire
                .write(&serde_json::json!({"ok": true, "family": 1, "n": 1, "result": [7]}))
                .expect("write");
            let back: serde_json::Value = Wire::read(&body).expect("read");
            assert_eq!(back["result"][0], serde_json::json!(7), "{wire:?}");
        }
    }

    #[test]
    fn both_encodings_carry_the_same_call() {
        let call = serde_json::json!({"call": "recall", "args": ["tests"]});
        let json: serde_json::Value = Wire::read(&Wire::Json.write(&call).expect("j")).expect("j");
        let cbor: serde_json::Value = Wire::read(&Wire::Cbor.write(&call).expect("c")).expect("c");
        assert_eq!(json, cbor, "one shape, two encodings");
    }
}
