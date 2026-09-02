//! The family's framing: four bytes of length, then JSON.
//!
//! Not [`axon_ipc`], which is what the UI and the daemon speak to each other. That is CBOR
//! inside an `Envelope` carrying a protocol version, and it is right for two halves of one
//! program that ship together — a peer from another build should be turned away at the
//! boundary rather than after its fields have been read.
//!
//! This socket is the opposite case. Anything may knock on it: another axon, a sibling tool,
//! somebody with `socat` working out why a message never arrived. So it speaks what the family
//! agreed and what [`super::wire`] documents — a big-endian `u32`, then a JSON body, and
//! nothing wrapped around it.
//!
//! **This was the bug this file exists to fix.** The socket was framed with `axon_ipc` and
//! documented as JSON, which is the failure the family's own guidance names first: it works
//! perfectly when axon talks to axon, and no sibling can say a word to it. Nothing inside the
//! tool that owns it can see that — every test passes, both ends agree — and it presents much
//! later as "that peer never answers".
//!
//! Both a blocking and an asynchronous half, because the two ends really are different: the
//! listener lives in a UI that must not block, and the caller is a tool peer whose whole job is
//! one round trip.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{Read, Write};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest frame either end will read or write.
///
/// A message between instances is a sentence, not a payload. Small on purpose: the socket is
/// reachable by anything running as this user, and an unbounded read is a way to make a session
/// allocate until it dies.
pub const MOST: usize = 1 << 20;

/// Encode one value, framed.
fn framed<T: Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    let body = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    if body.len() > MOST {
        return Err(std::io::Error::other("that is too much to say at once"));
    }
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// How long a frame says it is, or an error saying why it cannot be read.
fn expecting(header: [u8; 4]) -> std::io::Result<usize> {
    let len = u32::from_be_bytes(header) as usize;
    if len > MOST {
        return Err(std::io::Error::other(format!(
            "a frame of {len} bytes is beyond what this socket reads"
        )));
    }
    Ok(len)
}

/// Read one message from an async stream.
pub async fn read<T: DeserializeOwned, R: AsyncRead + Unpin>(from: &mut R) -> std::io::Result<T> {
    let mut header = [0_u8; 4];
    from.read_exact(&mut header).await?;
    let mut body = vec![0_u8; expecting(header)?];
    from.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// Write one message to an async stream.
pub async fn write<T: Serialize, W: AsyncWrite + Unpin>(
    to: &mut W,
    value: &T,
) -> std::io::Result<()> {
    to.write_all(&framed(value)?).await?;
    to.flush().await
}

/// Read one message from a blocking stream.
pub fn read_from<T: DeserializeOwned, R: Read>(from: &mut R) -> std::io::Result<T> {
    let mut header = [0_u8; 4];
    from.read_exact(&mut header)?;
    let mut body = vec![0_u8; expecting(header)?];
    from.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// Write one message to a blocking stream.
pub fn write_to<T: Serialize, W: Write>(to: &mut W, value: &T) -> std::io::Result<()> {
    to.write_all(&framed(value)?)?;
    to.flush()
}

/// What goes on the wire is what the documentation says goes on the wire.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::wire::{Call, Reply};

    #[test]
    fn a_frame_is_four_bytes_of_length_and_then_json() {
        // The whole point of the file. Somebody with `socat` and this documentation has to be
        // able to read what comes out, or the family contract is a comment.
        let out = framed(&Call {
            call: "status".to_owned(),
            ..Call::default()
        })
        .expect("frames");
        let len = u32::from_be_bytes([out[0], out[1], out[2], out[3]]) as usize;
        assert_eq!(len, out.len() - 4, "the length does not describe the body");
        let body = std::str::from_utf8(&out[4..]).expect("it is text");
        assert!(body.starts_with('{'), "{body}");
        assert!(body.contains(r#""call":"status""#), "{body}");
    }

    #[test]
    fn nothing_is_wrapped_around_the_body() {
        // No envelope, no version, no length repeated inside. A sibling reads the object it was
        // promised or the contract was never real.
        let out = framed(&Reply::done()).expect("frames");
        let body: serde_json::Value = serde_json::from_slice(&out[4..]).expect("decodes");
        let keys: Vec<&str> = body
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["ok", "n", "result"], "{body}");
    }

    #[test]
    fn a_message_survives_the_round_trip_blocking() {
        let sent = Call {
            call: "tell".to_owned(),
            args: vec![serde_json::json!("hello")],
            from: Some("axon/main/alpha-rho".to_owned()),
            token: None,
        };
        let mut wire = Vec::new();
        write_to(&mut wire, &sent).expect("writes");
        let back: Call = read_from(&mut wire.as_slice()).expect("reads");
        assert_eq!(sent, back);
    }

    #[tokio::test]
    async fn a_message_survives_the_round_trip_async() {
        let sent = Reply::of(serde_json::json!({"busy": false}));
        let mut wire = Vec::new();
        write(&mut wire, &sent).await.expect("writes");
        let back: Reply = read(&mut wire.as_slice()).await.expect("reads");
        assert_eq!(sent, back);
    }

    #[test]
    fn a_frame_claiming_more_than_the_cap_is_refused_before_anything_is_allocated() {
        // The socket is reachable by anything running as this user, and a length field is the
        // cheapest way to ask a session to allocate until it dies.
        let huge = u32::try_from(MOST + 1).expect("fits").to_be_bytes();
        assert!(expecting(huge).is_err());
        assert!(expecting(u32::MAX.to_be_bytes()).is_err());
    }

    #[test]
    fn a_body_that_is_not_json_is_an_error_rather_than_a_panic() {
        let mut wire = 3_u32.to_be_bytes().to_vec();
        wire.extend_from_slice(b"not");
        let read: std::io::Result<Call> = read_from(&mut wire.as_slice());
        assert!(read.is_err());
    }
}
