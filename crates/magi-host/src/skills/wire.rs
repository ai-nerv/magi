use super::{ENTRIES_MOST, Error, FILE_MOST, METADATA_MOST, valid_name};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

pub(super) const INPUT_MOST: usize = 32 * 1024;
pub(super) const REPLY_MOST: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub version: u16,
    pub root: PathBuf,
    pub operation: Operation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Operation {
    Catalog {},
    Load { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFailure {
    InvalidPath,
    NotFound,
    Unreadable,
    UnsafePath,
    Changed,
    TooManyEntries,
    Oversized,
    Metadata,
    Encoding,
    Timeout,
    Protocol,
    Worker,
    Cancelled,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    ok: bool,
    family: u16,
    n: usize,
    result: Vec<Response>,
    error: Option<String>,
    fault: Option<Fault>,
    surface: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Fault {
    Refused,
    Failed,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    version: u16,
    root: PathBuf,
    operation: Operation,
    result: Result<Payload, ReadFailure>,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum Payload {
    Catalog(Listing),
    Source(Text),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Listing {
    pub entries: Vec<Candidate>,
    pub invalid_names: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Candidate {
    pub name: String,
    pub metadata: Result<String, ReadFailure>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Text {
    pub text: String,
    pub sha256: String,
}

pub(super) fn path(root: &Path) -> Result<(), Error> {
    let text = root.to_str().ok_or(Error::Path)?;
    if !root.is_absolute()
        || text.len() > 4096
        || text.contains('\0')
        || root.components().count() > 256
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(Error::Path);
    }
    Ok(())
}

impl Request {
    pub(super) fn encode(&self) -> Result<Vec<u8>, Error> {
        path(&self.root)?;
        if self.version != 1 {
            return Err(Error::Protocol);
        }
        if let Operation::Load { name } = &self.operation
            && !valid_name(name)
        {
            return Err(Error::Document(super::Failure::Name));
        }
        let bytes = serde_json::to_vec(self).map_err(|_| Error::Protocol)?;
        if bytes.len() > INPUT_MOST {
            return Err(Error::Protocol);
        }
        Ok(bytes)
    }

    pub(super) fn decode(&self, bytes: &[u8]) -> Result<Payload, Error> {
        if bytes.len() > REPLY_MOST {
            return Err(Error::Protocol);
        }
        let reply: Reply = serde_json::from_slice(bytes).map_err(|_| Error::Protocol)?;
        if reply.family != 1 || reply.n != reply.result.len() || reply.surface.is_some() {
            return Err(Error::Protocol);
        }
        if !reply.ok {
            if reply.n != 0
                || reply.fault.is_none()
                || reply.error.as_ref().is_none_or(|text| text.len() > 4096)
            {
                return Err(Error::Protocol);
            }
            return Err(Error::Helper);
        }
        if reply.n != 1 || reply.fault.is_some() || reply.error.is_some() {
            return Err(Error::Protocol);
        }
        let response = reply.result.into_iter().next().ok_or(Error::Protocol)?;
        if response.version != 1
            || response.root != self.root
            || response.operation != self.operation
        {
            return Err(Error::Protocol);
        }
        let payload = response.result.map_err(Error::Read)?;
        match (&self.operation, &payload) {
            (Operation::Catalog {}, Payload::Catalog(listing)) => {
                if listing.entries.len().saturating_add(listing.invalid_names) > ENTRIES_MOST {
                    return Err(Error::Protocol);
                }
                let mut previous: Option<&str> = None;
                for entry in &listing.entries {
                    if !valid_name(&entry.name)
                        || previous.is_some_and(|p| p >= entry.name.as_str())
                    {
                        return Err(Error::Protocol);
                    }
                    if entry
                        .metadata
                        .as_ref()
                        .is_ok_and(|text| text.len() > METADATA_MOST || text.contains('\0'))
                    {
                        return Err(Error::Protocol);
                    }
                    previous = Some(&entry.name);
                }
            }
            (Operation::Load { .. }, Payload::Source(source)) => {
                use sha2::{Digest, Sha256};
                if source.text.len() > FILE_MOST
                    || source.text.contains('\0')
                    || source.sha256 != format!("{:x}", Sha256::digest(source.text.as_bytes()))
                {
                    return Err(Error::Protocol);
                }
            }
            _ => return Err(Error::Protocol),
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests;
