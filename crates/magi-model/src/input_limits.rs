use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_bytes: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<ImageInputLimits>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageInputLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_message: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_request: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resize: Option<ImageResize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageResize {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<NonZeroU64>,
    /// Maximum bytes in the base64 payload, excluding any data-URL prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<NonZeroU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jpeg_quality: Option<JpegQuality>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct JpegQuality(u8);

impl TryFrom<u8> for JpegQuality {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if (1..=100).contains(&value) {
            Ok(Self(value))
        } else {
            Err("JPEG quality must be between 1 and 100")
        }
    }
}

impl From<JpegQuality> for u8 {
    fn from(value: JpegQuality) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests;
