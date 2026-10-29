use anyhow::{Context, Result, ensure};
use magi_proto::{Cursor, SessionId};
use std::path::Path;

mod delivery;
mod paging;
pub(super) use delivery::{Delivery, Replay};

const PREFIX: &str = "magi-v1-";
const NATIVE: usize = 512;
pub(super) const ID_BYTES: usize = PREFIX.len() + NATIVE * 2;

pub(super) fn external(transcript: &str) -> Result<String> {
    use std::fmt::Write;
    ensure!(
        !transcript.is_empty()
            && transcript.len() <= NATIVE
            && !transcript.chars().any(char::is_control),
        "invalid native transcript identity"
    );
    let mut id = String::with_capacity(PREFIX.len() + transcript.len() * 2);
    id.push_str(PREFIX);
    for byte in transcript.bytes() {
        write!(&mut id, "{byte:02x}")?;
    }
    Ok(id)
}

pub(super) fn native(id: &str) -> Result<String> {
    ensure!(id.len() <= ID_BYTES, "session identity exceeds its limit");
    let hex = id
        .strip_prefix(PREFIX)
        .context("unknown session identity version")?;
    ensure!(
        !hex.is_empty()
            && hex.len().is_multiple_of(2)
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid session identity encoding"
    );
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
        .collect::<Result<Vec<_>>>()?;
    let transcript = String::from_utf8(bytes).context("invalid session identity text")?;
    ensure!(
        external(&transcript)? == id,
        "noncanonical session identity"
    );
    Ok(transcript)
}

pub(super) fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= ID_BYTES && !id.chars().any(char::is_control),
        "invalid session identity"
    );
    if id.starts_with(PREFIX) {
        native(id)?;
    }
    Ok(())
}

pub(super) async fn identity(
    socket: &Path,
    key: String,
    session: &SessionId,
    through: Cursor,
) -> Result<String> {
    let page = paging::Paging::new(session.clone(), through, through, None)?
        .read(socket, &key)
        .await?
        .context("missing native history identity page")?;
    external(
        page.transcript
            .as_deref()
            .context("native transcript identity unavailable")?,
    )
}

#[cfg(test)]
mod tests;
