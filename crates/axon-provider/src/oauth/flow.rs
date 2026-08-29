//! The sign-in itself: proof key, browser, callback, exchange.
//!
//! Authorization code with PKCE, because axon is a program on your machine and not a server:
//! it cannot keep a client secret, and a public client without a proof key can have its
//! callback intercepted by anything else on the machine that registered the same URL scheme.
//! The proof key is what makes the intercepted code useless.
//!
//! The callback comes back to `127.0.0.1` on a port the operating system chooses. Loopback
//! rather than a custom URL scheme: nothing else can claim it, it needs no registration, and
//! the port being unpredictable is one more thing an interceptor would have to guess.

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The proof a sign-in is finished by whoever started it.
#[derive(Debug, Clone)]
pub struct Pkce {
    /// The secret, kept until the code is exchanged.
    pub verifier: String,
    /// Its hash, which is what the authorization server sees.
    pub challenge: String,
}

impl Pkce {
    /// Generate a fresh pair.
    ///
    /// # Errors
    ///
    /// If the system has no randomness to give, which is not a case to paper over: a
    /// predictable verifier is no verifier.
    pub fn generate() -> Result<Self, super::Error> {
        let mut bytes = [0_u8; 32];
        std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut bytes)?;
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            verifier,
            challenge,
        })
    }
}

/// Where to send the browser.
#[must_use]
pub fn authorize_url(
    endpoint: &str,
    client_id: &str,
    redirect: &str,
    scopes: &[String],
    pkce: &Pkce,
    state: &str,
) -> String {
    let joined = scopes.join(" ");
    let query = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect),
        ("scope", &joined),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={}", encode(v)))
    .collect::<Vec<_>>()
    .join("&");
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{separator}{query}")
}

/// Percent-encode everything that is not unreserved.
///
/// Written out rather than pulled in: the unreserved set is four lines of RFC 3986 and the
/// alternative is a dependency whose whole job is those four lines.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// What the browser came back with.
#[derive(Debug, PartialEq, Eq)]
pub struct Callback {
    /// The authorization code, to be exchanged.
    pub code: String,
    /// The value the request was started with, which must match.
    pub state: String,
}

/// Wait on `listener` for the browser's redirect, and answer it.
///
/// Blocking and single-shot: there is exactly one sign-in in flight and nothing else should be
/// talking to this port. The reply is a page rather than a bare status because the person is
/// looking at a browser window and deserves to be told they can close it.
///
/// # Errors
///
/// If the connection fails, or the request carries an error instead of a code — which is what
/// arrives when somebody clicks "deny".
pub fn listen_for_code(listener: &std::net::TcpListener) -> Result<Callback, super::Error> {
    use std::io::{BufRead, BufReader, Write};

    let (mut socket, _) = listener.accept()?;
    let mut line = String::new();
    BufReader::new(socket.try_clone()?).read_line(&mut line)?;

    // `GET /?code=...&state=... HTTP/1.1`
    let target = line.split_whitespace().nth(1).unwrap_or_default();
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or_default();
    let mut code = None;
    let mut state = None;
    let mut denied = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "code" => code = Some(decode(value)),
            "state" => state = Some(decode(value)),
            "error" => denied = Some(decode(value)),
            _ => {}
        }
    }

    let answer = if code.is_some() {
        "Signed in. You can close this window."
    } else {
        "Sign-in failed. You can close this window."
    };
    let _ = socket.write_all(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{answer}",
            answer.len()
        )
        .as_bytes(),
    );

    if let Some(why) = denied {
        return Err(super::Error::Refused(why));
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok(Callback { code, state }),
        _ => Err(super::Error::Refused(
            "the callback carried no authorization code".to_owned(),
        )),
    }
}

/// Undo percent-encoding, and `+` for a space.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or_default();
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a token endpoint replies with.
#[derive(Debug, Deserialize)]
struct Granted {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Seconds from now, which is how every one of them reports it.
    #[serde(default)]
    expires_in: Option<u64>,
}

/// How long a token is assumed to last when the provider does not say.
///
/// Short on purpose: guessing long means using a dead token and losing a turn, and guessing
/// short costs one refresh.
const ASSUMED_LIFETIME: u64 = 3600;

/// Turn an authorization code, or a refresh token, into tokens.
///
/// One function for both because they are the same request with a different grant, and two
/// would be two places for the parsing of one reply to drift.
///
/// # Errors
///
/// If the endpoint cannot be reached, or refuses.
pub async fn exchange(
    http: &reqwest::Client,
    token_url: &str,
    form: &[(&str, &str)],
) -> Result<super::Tokens, super::Error> {
    let response = http
        .post(token_url)
        .form(form)
        .send()
        .await
        .map_err(|e| super::Error::Refused(e.to_string()))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(super::Error::Refused(format!(
            "the token endpoint returned {status}: {}",
            body.lines().next().unwrap_or_default()
        )));
    }
    let granted: Granted = serde_json::from_str(&body)
        .map_err(|e| super::Error::Refused(format!("unreadable token reply: {e}")))?;
    Ok(super::Tokens {
        access: granted.access_token,
        refresh: granted.refresh_token,
        expires_at: super::now() + granted.expires_in.unwrap_or(ASSUMED_LIFETIME),
    })
}
