//! A whole turn, against a server that speaks the protocol.
//!
//! No account and no network: a local listener answers with recorded Anthropic SSE, so the
//! path a real turn takes — request built, stream parsed, deltas folded, entry amended,
//! journal written — is exercised end to end.

use axum_host::session::Session;
use axum_host::turn::{Backend, run};
use axum_lua::adapter::{LuaAdapter, engine_with_builtins};
use axum_proto::{Entry, SessionId, StopReason};
use axum_provider::api::Options;
use axum_provider::client::Client;
use axum_provider::model::{Api, Modality, Model};
use axum_provider::provider::{Auth, Provider};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;

/// A recorded Anthropic response: thinking, text, then a clean stop.
const STREAM: &str = "\
event: message_start\n\
data: {\"message\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":0}}}\n\
\n\
event: content_block_start\n\
data: {\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\
\n\
event: content_block_delta\n\
data: {\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"weighing it\"}}\n\
\n\
event: content_block_delta\n\
data: {\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-abc\"}}\n\
\n\
event: content_block_stop\n\
data: {\"index\":0}\n\
\n\
event: content_block_start\n\
data: {\"index\":1,\"content_block\":{\"type\":\"text\"}}\n\
\n\
event: content_block_delta\n\
data: {\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"The journal \"}}\n\
\n\
event: content_block_delta\n\
data: {\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"is append-only.\"}}\n\
\n\
event: message_delta\n\
data: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\
\n";

/// Serve one HTTP response carrying `body`, then stop.
fn serve_once(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        // Drain the request headers so the client's write completes before we answer.
        let mut reader = BufReader::new(socket.try_clone().expect("clone"));
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes());
    });
    format!("http://127.0.0.1:{port}")
}

fn backend(base_url: String) -> Backend {
    let model = Model {
        id: "claude-sonnet-4-5".into(),
        name: "Claude Sonnet 4.5".into(),
        provider: "fake".into(),
        api: Api::AnthropicMessages,
        reasoning: true,
        input: vec![Modality::Text],
        context_window: 200_000,
        max_tokens: 8192,
        cost: axum_model::Cost::default(),
        thinking: BTreeMap::new(),
        compat: None,
    };
    Backend {
        provider: Provider {
            id: "fake".into(),
            name: "Fake".into(),
            base_url: Some(base_url),
            api: Api::AnthropicMessages,
            auth: Auth::None,
            compat: None,
            models: vec![model.clone()],
        },
        model,
        options: Options::default(),
    }
}

fn session(name: &str) -> (tokio::sync::Mutex<Session>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("axum-turn-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("s.jsonl");
    let session = Session::open(&path, SessionId::new("s"), "/tmp", 0).expect("session");
    (tokio::sync::Mutex::new(session), dir)
}

#[tokio::test]
async fn a_turn_streams_into_the_journal() {
    let (session, dir) = session("ok");
    let backend = backend(serve_once("200 OK", STREAM));

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    run(&session, &backend, &adapter, &Client::new())
        .await
        .expect("the turn runs");

    let held = session.lock().await;
    let entries = held.entries();
    assert_eq!(entries.len(), 1, "one assistant entry, amended in place");
    let Entry::Assistant {
        text,
        thinking,
        stop_reason,
        error,
        ..
    } = &entries[0]
    else {
        panic!("expected an assistant entry, got {:?}", entries[0]);
    };
    assert_eq!(text, "The journal is append-only.");
    assert_eq!(thinking, "weighing it");
    assert_eq!(*stop_reason, Some(StopReason::EndTurn));
    assert!(error.is_none());

    // The journal holds it too, not just the in-memory transcript.
    let source = std::fs::read_to_string(dir.join("s.jsonl")).expect("journal");
    assert!(source.contains("append-only"), "the turn reached the disk");

    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_provider_error_becomes_a_well_formed_entry() {
    // Errors are values: the transcript stays uniform and the UI needs no error branch.
    let (session, dir) = session("err");
    let backend = backend(serve_once("529 Overloaded", "{\"error\":\"overloaded\"}"));

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    run(&session, &backend, &adapter, &Client::new())
        .await
        .expect("the turn returns");

    let held = session.lock().await;
    let Entry::Assistant {
        stop_reason, error, ..
    } = &held.entries()[0]
    else {
        panic!("expected an assistant entry");
    };
    assert_eq!(*stop_reason, Some(StopReason::Error));
    assert!(
        error.as_deref().unwrap_or_default().contains("529"),
        "{error:?}"
    );

    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_turn_ends_idle_whatever_happened() {
    // A status that never changes is indistinguishable from a hang.
    let (session, dir) = session("idle");
    let backend = backend(serve_once("500 Server Error", "boom"));

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    run(&session, &backend, &adapter, &Client::new())
        .await
        .expect("the turn returns");

    assert_eq!(
        *session.lock().await.status(),
        axum_proto::AgentStatus::Idle
    );
    let _ = std::fs::remove_dir_all(&dir);
}
