//! Transient provider serialization observations, without request content.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFrame {
    pub version: u8,
    pub dialect: [u8; 32],
    pub policy: [u8; 32],
    pub omitted_images: u64,
}
