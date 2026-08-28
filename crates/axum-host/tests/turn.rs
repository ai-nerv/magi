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
        // Empty: this test builds its own adapter directly, and a worker is not involved.
        apis: Vec::new(),
        tools: Vec::new(),
        stubs: Vec::new(),
        cwd: std::env::temp_dir(),
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
        system: None,
        confine: false,
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
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    run(
        &session,
        &backend,
        &adapter,
        &Client::with_base_delay(std::time::Duration::from_millis(1)),
        &registry,
        &ops,
    )
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
    // Every attempt, not one: with retries a one-shot server answers the first and refuses
    // the connection for the rest, so the failure reported would be a transport error rather
    // than the 529 this is about.
    let (url, _seen) = serve_flaky(usize::MAX, "529 Overloaded", STREAM);
    let backend = backend(url);

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    run(
        &session,
        &backend,
        &adapter,
        &Client::with_base_delay(std::time::Duration::from_millis(1)),
        &registry,
        &ops,
    )
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
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    run(
        &session,
        &backend,
        &adapter,
        &Client::with_base_delay(std::time::Duration::from_millis(1)),
        &registry,
        &ops,
    )
    .await
    .expect("the turn returns");

    assert_eq!(
        *session.lock().await.status(),
        axum_proto::AgentStatus::Idle
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Accept a connection and never answer, standing in for a model still composing its reply.
fn serve_never() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let held = listener.accept();
        // Held open: dropping the socket would close the stream and end the turn by itself,
        // which is the thing this test must not be able to mistake for a cancellation.
        std::thread::sleep(std::time::Duration::from_secs(30));
        drop(held);
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn an_interrupt_stops_a_turn_the_model_has_not_finished() {
    let (session, dir) = session("cancel");
    let backend = backend(serve_never());

    let cancel = session.lock().await.cancel();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        cancel.request();
    });

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        run(
            &session,
            &backend,
            &adapter,
            &Client::with_base_delay(std::time::Duration::from_millis(1)),
            &registry,
            &ops,
        ),
    )
    .await
    .expect("the turn gives up rather than waiting out the provider")
    .expect("the turn returns");

    let held = session.lock().await;
    let Entry::Assistant {
        stop_reason, error, ..
    } = &held.entries()[0]
    else {
        panic!("expected an assistant entry");
    };
    // Aborted, not Error: the user stopped it, and nothing went wrong.
    assert_eq!(*stop_reason, Some(StopReason::Aborted));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(*held.status(), axum_proto::AgentStatus::Idle);

    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Refuse the first request the way a provider refuses an over-long one, then behave.
///
/// A 400 with the reason in the body, which is how every one of them says it: the status alone
/// cannot tell "too long" from "malformed", and only one of those is worth compacting for.
fn serve_overflow_then(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for (served, socket) in listener.incoming().enumerate() {
            let Ok(mut socket) = socket else { return };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(socket.try_clone().expect("clone"));
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    line.clear();
                }
                let (status, payload) = if served == 0 {
                    (
                        "400 Bad Request",
                        "{\"error\":\"prompt is too long: 300000 tokens > 200000 maximum\"}",
                    )
                } else {
                    ("200 OK", body)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes());
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn an_overflow_is_compacted_and_the_turn_carries_on() {
    // The failure that ends a long session. `Overflow` was a class nothing ever produced,
    // because the window arriving as a plain 400 made it indistinguishable from a bug.
    let (session, dir) = session("overflow");
    let backend = backend(serve_overflow_then(STREAM));

    // Long enough to have something to summarise; `covers` declines below that.
    {
        let mut held = session.lock().await;
        for i in 0..12 {
            held.commit(Entry::User {
                id: axum_proto::MessageId::new(format!("u{i}")),
                text: format!("message number {i}"),
            })
            .expect("commit");
        }
    }

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    run(
        &session,
        &backend,
        &adapter,
        &Client::with_base_delay(std::time::Duration::from_millis(1)),
        &registry,
        &ops,
    )
    .await
    .expect("the turn runs");

    let held = session.lock().await;
    let entries = held.entries();
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Compaction { .. })),
        "the conversation was compacted: {entries:?}"
    );
    let last = entries.last().expect("an entry");
    let Entry::Assistant { text, .. } = last else {
        panic!("expected the retried answer, got {last:?}");
    };
    assert_eq!(text, "The journal is append-only.");

    // The refusal stays in the transcript. A reader noticing the model forget something needs
    // to be able to see that this is why.
    assert!(
        entries.iter().any(|e| matches!(
            e,
            Entry::Assistant { error: Some(why), .. } if why.contains("too long")
        )),
        "{entries:?}"
    );
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_compacted_session_sends_the_summary_and_not_the_history() {
    // What compaction is for. The point is not that a record exists; it is that the next
    // request is smaller and still says what the task was.
    let (session, dir) = session("compacted-context");
    {
        let mut held = session.lock().await;
        for i in 0..12 {
            held.commit(Entry::User {
                id: axum_proto::MessageId::new(format!("u{i}")),
                text: format!("forgotten message {i}"),
            })
            .expect("commit");
        }
        held.commit(Entry::Compaction {
            id: axum_proto::MessageId::new("k1"),
            summary: "The user is porting a journal to Rust.".into(),
            replaces: 10,
        })
        .expect("commit");
        held.commit(Entry::User {
            id: axum_proto::MessageId::new("u99"),
            text: "carry on".into(),
        })
        .expect("commit");
    }

    let held = session.lock().await;
    let context = axum_host::context::of(&held);
    let sent = format!("{:?}", context.messages);
    assert!(sent.contains("porting a journal"), "the summary is sent");
    assert!(sent.contains("carry on"), "and what followed it");
    assert!(
        !sent.contains("forgotten message 0"),
        "but not what it replaced: {sent}"
    );
    // And the tail it deliberately kept. Starting from the compaction record rather than from
    // `replaces` threw this away — the recent turns are the whole reason the tail is kept.
    assert!(
        sent.contains("forgotten message 10") && sent.contains("forgotten message 11"),
        "the kept tail survives: {sent}"
    );
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fail the first `failures` requests with `status`, then behave.
///
/// Returns the endpoint and a count of how many requests it was sent. Counted rather than
/// timed, because these tests pause the clock and a paused clock auto-advances whenever the
/// runtime has nothing to run — which, while a real socket is in flight, is most of the time.
/// Elapsed virtual time therefore says nothing about whether anything waited.
fn serve_flaky(
    failures: usize,
    status: &'static str,
    body: &'static str,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = std::sync::Arc::clone(&seen);
    std::thread::spawn(move || {
        for (served, socket) in listener.incoming().enumerate() {
            let Ok(mut socket) = socket else { return };
            let counter = std::sync::Arc::clone(&counter);
            std::thread::spawn(move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut reader = BufReader::new(socket.try_clone().expect("clone"));
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    line.clear();
                }
                let (status, payload) = if served < failures {
                    (status, "{\"error\":\"overloaded\"}")
                } else {
                    ("200 OK", body)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes());
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

#[tokio::test]
async fn a_provider_having_a_bad_moment_is_waited_out() {
    // `retry.rs` has had classification, exponential backoff, per-request jitter and a full
    // test suite since M2, and nothing called any of it: a 529 — routine from a busy provider
    // — killed the turn outright while the code that would have survived it sat one module
    // away. The client is given a one-millisecond base delay, so the policy runs in full and
    // the test does not spend a minute proving arithmetic that has its own tests.
    let (session, dir) = session("flaky");
    let (url, seen) = serve_flaky(2, "529 Overloaded", STREAM);
    let backend = backend(url);

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    run(
        &session,
        &backend,
        &adapter,
        &Client::with_base_delay(std::time::Duration::from_millis(1)),
        &registry,
        &ops,
    )
    .await
    .expect("the turn runs");

    let held = session.lock().await;
    let Entry::Assistant { text, error, .. } = &held.entries()[0] else {
        panic!("expected an assistant entry");
    };
    assert_eq!(text, "The journal is append-only.", "it got through");
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        seen.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "two refusals and the one that worked"
    );
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failure_that_retrying_cannot_fix_is_not_retried() {
    // A 401 is not a bad moment. Waiting it out four times wastes a minute to arrive at the
    // same answer, and the answer is one only a person can act on.
    let (session, dir) = session("auth");
    let (url, seen) = serve_flaky(usize::MAX, "401 Unauthorized", STREAM);
    let backend = backend(url);

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    run(
        &session,
        &backend,
        &adapter,
        &Client::with_base_delay(std::time::Duration::from_millis(1)),
        &registry,
        &ops,
    )
    .await
    .expect("the turn returns");

    assert_eq!(
        seen.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "asked once and took the answer"
    );
    let held = session.lock().await;
    let Entry::Assistant { error, .. } = &held.entries()[0] else {
        panic!("expected an assistant entry");
    };
    assert!(
        error.as_deref().unwrap_or_default().contains("401"),
        "{error:?}"
    );
    drop(held);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_wait_is_announced_while_it_is_happening() {
    // The UI has had a `Retrying` display since M0 that nothing ever set. Saying so afterwards
    // is no use: the point is to tell a person watching a spinner that it is a wait.
    let (session, dir) = session("announced");
    let (url, _seen) = serve_flaky(1, "529 Overloaded", STREAM);
    let backend = backend(url);
    let mut live = session.lock().await.subscribe();

    let adapter = LuaAdapter::new(
        engine_with_builtins().expect("builtins"),
        "anthropic-messages",
    )
    .expect("the protocol is registered");
    let registry = axum_tools::Registry::new();
    let ops = axum_tools::ops::Real::new(std::env::temp_dir());
    run(
        &session,
        &backend,
        &adapter,
        &Client::with_base_delay(std::time::Duration::from_millis(1)),
        &registry,
        &ops,
    )
    .await
    .expect("the turn runs");

    let mut announced = None;
    while let Ok(event) = live.try_recv() {
        if let axum_proto::HarnessEvent::StatusChanged {
            status:
                axum_proto::AgentStatus::Retrying {
                    attempt, delay_ms, ..
                },
            ..
        } = event
        {
            announced = Some((attempt, delay_ms));
        }
    }
    let (attempt, delay_ms) = announced.expect("the wait was published");
    assert_eq!(attempt, 1, "the first try is the one that failed");
    assert!(delay_ms > 0, "and it says how long");
    drop(live);
    let _ = std::fs::remove_dir_all(&dir);
}
