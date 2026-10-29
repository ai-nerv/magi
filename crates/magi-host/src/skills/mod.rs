//! Bounded workflow documents and explicit invocation syntax.

mod catalog;
mod client;
mod frontmatter;
mod invocation;
pub(crate) mod snapshot;
mod wire;

pub use catalog::{
    Catalog, CatalogEntry, Diagnostic, DiscoveryError, Loaded, Roots, Source, SourceClass,
};
pub use client::Client;
pub use frontmatter::{metadata, parse};
pub use invocation::{Invocation, invocation};
pub use wire::ReadFailure;

pub const FILE_MOST: usize = 64 * 1024;
pub const METADATA_MOST: usize = 4 * 1024;
pub const ENTRIES_MOST: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("workflow root must be a bounded absolute path without parent traversal")]
    Path,
    #[error("workflow helper returned an invalid or unsupported reply")]
    Protocol,
    #[error("workflow acquisition was cancelled")]
    Cancelled,
    #[error("workflow helper deadline elapsed")]
    Timeout,
    #[error("workflow helper could not start or finish successfully")]
    Helper,
    #[error("workflow helper does not match the configured program pin")]
    Pin,
    #[error("workflow acquisition failed: {0:?}")]
    Read(ReadFailure),
    #[error("workflow document is invalid: {0}")]
    Document(Failure),
    #[error("workflow is not in the selected catalog")]
    Missing,
    #[error("workflow metadata changed; refresh the catalog before selecting it")]
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub name: String,
    pub description: String,
    pub body: String,
    pub source_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    #[error("workflow file exceeds 64 KiB")]
    Oversized,
    #[error("workflow metadata exceeds 4 KiB")]
    MetadataOversized,
    #[error("workflow is not valid UTF-8")]
    Encoding,
    #[error("workflow metadata must contain only a unique name and description")]
    Metadata,
    #[error(
        "workflow name must match its directory and use at most 64 lowercase ASCII letters, digits or hyphens, with alphanumeric ends"
    )]
    Name,
    #[error("workflow body is empty or contains NUL")]
    Body,
    #[error("use /skill <name> [literal arguments]")]
    Invocation,
}

pub fn valid_name(name: &str) -> bool {
    let alphanumeric = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| alphanumeric(b) || b == b'-')
        && name.bytes().next().is_some_and(alphanumeric)
        && name.bytes().next_back().is_some_and(alphanumeric)
}
