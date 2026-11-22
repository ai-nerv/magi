use magi_model::{Image, ImageResize};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

mod codec;
mod history;
mod process;
pub(crate) use history::History;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Profile {
    pub model: Option<String>,
    pub resize: Option<ImageResize>,
}

pub const WORKER_ARG: &str = "--image-worker-v1";
pub(crate) const WIRE_BYTES: usize = 1024 * 1024;
const INLINE_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Normalize,
    Validate,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    mode: Mode,
    profile: ImageResize,
    images: Vec<Image>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Failure {
    Invalid,
    Unsupported,
    Resource,
    History,
    CannotFit,
}

impl Failure {
    fn message(self) -> &'static str {
        match self {
            Self::Invalid => "Image data or image-worker request is invalid.",
            Self::Unsupported => "Image encoding does not match a supported raster format.",
            Self::Resource => "Image exceeds the normalization resource limits.",
            Self::History => {
                "A historical image exceeds the selected model's resize profile; history was not changed."
            }
            Self::CannotFit => "Image cannot fit the selected model's resize profile.",
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Ok { version: u8, images: Vec<Image> },
    Failed { version: u8, error: Failure },
}

/// Normalize only the supplied new images; no history is read or changed.
pub async fn normalize(images: Vec<Image>, profile: &ImageResize) -> Result<Vec<Image>, String> {
    process::run(Request {
        version: 1,
        mode: Mode::Normalize,
        profile: profile.clone(),
        images,
    })
    .await
}

/// Check the supplied immutable images against the selected resize profile.
pub async fn validate(images: &[Image], profile: &ImageResize) -> Result<(), String> {
    let checked = process::run(Request {
        version: 1,
        mode: Mode::Validate,
        profile: profile.clone(),
        images: images.to_vec(),
    })
    .await?;
    if checked != images {
        return Err("Image validator changed historical image bytes.".into());
    }
    Ok(())
}

/// Run one bounded inline-image operation on standard input and output, without configuration.
pub fn worker() -> Result<(), String> {
    restrict().map_err(|_| "Could not apply image-worker resource limits.")?;
    let result = (|| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take((WIRE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::Invalid)?;
        if bytes.len() > WIRE_BYTES {
            return Err(Failure::Resource);
        }
        let request: Request = serde_json::from_slice(&bytes).map_err(|_| Failure::Invalid)?;
        codec::run(request)
    })();
    let reply = match result {
        Ok(images) => Reply::Ok { version: 1, images },
        Err(error) => Reply::Failed { version: 1, error },
    };
    let bytes = serde_json::to_vec(&reply).map_err(|_| "Could not encode image-worker result.")?;
    if bytes.len() > WIRE_BYTES {
        return Err("Image-worker reply exceeds its byte limit.".into());
    }
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| "Could not write image-worker result.".into())
}

fn restrict() -> rustix::io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    for (resource, cap) in [
        (Resource::As, 256 * 1024 * 1024),
        (Resource::Cpu, 4),
        (Resource::Core, 0),
        (Resource::Fsize, 0),
    ] {
        let existing = getrlimit(resource);
        let cap = existing
            .current
            .into_iter()
            .chain(existing.maximum)
            .fold(cap, u64::min);
        setrlimit(
            resource,
            Rlimit {
                current: Some(cap),
                maximum: Some(cap),
            },
        )?;
    }
    Ok(())
}
