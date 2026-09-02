//! Signing in, against an authorization server that behaves like a real one.
//!
//! The browser leg cannot be automated, so the test *is* the browser: it reads the URL magi
//! would have opened and calls back the way a redirect does. Everything either side of that —
//! the proof key, the encoding, the exchange, the file mode — is the part that can be wrong
//! silently, and is what these cover.

use magi_provider::oauth::{Pkce, Store, Tokens, authorize_url, exchange, listen_for_code};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

/// A token endpoint that records what it was sent.
fn token_endpoint(
    reply: &'static str,
    status: &'static str,
) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (seen, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { return };
            let mut reader = BufReader::new(socket.try_clone().expect("clone"));
            let mut line = String::new();
            let mut length = 0usize;
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let _ = seen.send(String::from_utf8_lossy(&body).into_owned());
            let _ = socket.write_all(
                format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .as_bytes(),
            );
        }
    });
    (format!("http://127.0.0.1:{port}"), heard)
}

/// Act as the browser: fetch the callback URL the way a redirect would.
fn visit(url: &str) {
    let rest = url.strip_prefix("http://").expect("a loopback url");
    let (host, target) = rest.split_once('/').unwrap_or((rest, ""));
    let Ok(mut socket) = std::net::TcpStream::connect(host) else {
        return;
    };
    let _ = socket.write_all(
        format!("GET /{target} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n\r\n").as_bytes(),
    );
    let mut sink = String::new();
    let _ = socket.read_to_string(&mut sink);
}

#[test]
fn a_proof_key_is_unpredictable_and_its_challenge_is_the_hash() {
    let one = Pkce::generate().expect("randomness");
    let two = Pkce::generate().expect("randomness");
    assert_ne!(one.verifier, two.verifier, "a predictable verifier is none");
    assert_ne!(
        one.challenge, one.verifier,
        "the server must not see the secret"
    );
    // S256, url-safe and unpadded: `+`, `/` and `=` all change meaning in a query string.
    assert!(!one.challenge.contains('+') && !one.challenge.contains('/'));
    assert!(!one.challenge.contains('='), "{}", one.challenge);
}

#[test]
fn the_authorize_url_carries_everything_the_server_needs() {
    let pkce = Pkce::generate().expect("randomness");
    let url = authorize_url(
        "https://example.test/authorize",
        "client-123",
        "http://127.0.0.1:9999",
        &["a:b".to_owned(), "c:d".to_owned()],
        &pkce,
        "state-abc",
    );
    assert!(url.contains("response_type=code"));
    assert!(url.contains("client_id=client-123"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains(&format!("code_challenge={}", pkce.challenge)));
    assert!(url.contains("state=state-abc"));
    // Encoded, not passed through: a space and a colon both change meaning in a query.
    assert!(url.contains("scope=a%3Ab%20c%3Ad"), "{url}");
    assert!(
        url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A9999"),
        "{url}"
    );
}

#[test]
fn an_endpoint_that_already_has_a_query_gets_an_ampersand() {
    let pkce = Pkce::generate().expect("randomness");
    let url = authorize_url(
        "https://example.test/authorize?tenant=acme",
        "c",
        "r",
        &[],
        &pkce,
        "s",
    );
    assert!(url.contains("?tenant=acme&response_type=code"), "{url}");
}

#[test]
fn the_callback_hands_back_the_code_and_the_state() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || visit(&format!("http://127.0.0.1:{port}/?code=abc123&state=xyz")));

    let callback = listen_for_code(&listener).expect("a code");
    assert_eq!(callback.code, "abc123");
    assert_eq!(callback.state, "xyz");
}

#[test]
fn a_percent_encoded_code_comes_back_decoded() {
    // Codes are opaque and routinely contain characters a query must escape. Handing the
    // escaped form to the token endpoint is a refusal that looks like a wrong password.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || visit(&format!("http://127.0.0.1:{port}/?code=a%2Fb%2Bc&state=s")));

    let callback = listen_for_code(&listener).expect("a code");
    assert_eq!(callback.code, "a/b+c");
}

#[test]
fn clicking_deny_is_reported_as_a_refusal() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        visit(&format!(
            "http://127.0.0.1:{port}/?error=access_denied&state=s"
        ));
    });

    let why = listen_for_code(&listener).expect_err("denied");
    assert!(why.to_string().contains("access_denied"), "{why}");
}

#[tokio::test]
async fn exchanging_a_code_sends_the_verifier_and_stores_what_comes_back() {
    let (url, heard) = token_endpoint(
        r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":900}"#,
        "200 OK",
    );
    let tokens = exchange(
        &reqwest::Client::new(),
        &url,
        &[
            ("grant_type", "authorization_code"),
            ("code", "abc123"),
            ("code_verifier", "the-secret"),
        ],
    )
    .await
    .expect("tokens");

    let sent = heard.recv().expect("a request");
    assert!(sent.contains("code_verifier=the-secret"), "{sent}");
    assert!(sent.contains("grant_type=authorization_code"), "{sent}");
    assert_eq!(tokens.access, "at-1");
    assert_eq!(tokens.refresh.as_deref(), Some("rt-1"));
    assert!(tokens.expires_at > magi_provider::oauth::now());
}

#[tokio::test]
async fn a_reply_with_no_expiry_is_given_a_short_one() {
    // Assuming a long life means using a dead token and losing a turn; assuming a short one
    // costs a refresh.
    let (url, _heard) = token_endpoint(r#"{"access_token":"at"}"#, "200 OK");
    let tokens = exchange(&reqwest::Client::new(), &url, &[])
        .await
        .expect("tokens");
    let life = tokens.expires_at - magi_provider::oauth::now();
    assert!(life <= 3600, "{life}");
}

#[tokio::test]
async fn a_refusal_says_what_the_endpoint_said() {
    let (url, _heard) = token_endpoint(r#"{"error":"invalid_grant"}"#, "400 Bad Request");
    let why = exchange(&reqwest::Client::new(), &url, &[])
        .await
        .expect_err("refused");
    assert!(why.to_string().contains("invalid_grant"), "{why}");
}

#[test]
fn the_store_is_written_readable_only_by_its_owner() {
    // A token that was briefly world-readable has been read, and nothing later takes that back.
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("magi-oauth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("credentials.json");

    let mut store = Store::default();
    store.put(
        "p",
        Tokens {
            access: "secret".into(),
            refresh: None,
            expires_at: 1,
        },
    );
    store.save_to(&path).expect("save");

    let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{:o}", mode & 0o777);

    let back = Store::load_from(&path).expect("load");
    assert_eq!(back.get("p").expect("tokens").access, "secret");
    let _ = std::fs::remove_dir_all(&dir);
}
