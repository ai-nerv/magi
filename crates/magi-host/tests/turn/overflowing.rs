//! After a provider refuses a request as too long, what goes next is smaller than what was refused,
//! or nothing goes: against a memory layer that answers `overflowed` with whatever it is told to.

use super::{backend, session};
use magi_host::scribe::Scribe;
use magi_ipc::family::{Family, Reply};
use magi_model::scratch::Scratch;
use magi_proto::{Entry, MessageId, SessionId, StopReason, ToolCallId, ToolResult};
use magi_testkit::Mind;
use magi_testkit::mind::{failed_line, stop_line, text_line};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What a memory layer answers `overflowed` with, from the cursors the last layout was asked for.
type Tighter = fn(&[u64]) -> Vec<Value>;

/// A memory layer that lays out everything live, and answers a refusal with `tighter`.
struct Memory {
    dir: Scratch,
}

impl Memory {
    fn start(name: &str, tighter: Tighter) -> Self {
        let dir = Scratch::new("magi-overflowing", name);
        let listener = tokio::net::UnixListener::bind(dir.join("m.sock")).expect("bind");
        let live: Arc<Mutex<Vec<u64>>> = Arc::default();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let live = Arc::clone(&live);
                tokio::spawn(async move {
                    while let Ok(length) = stream.read_u32().await {
                        let mut body = vec![0; length as usize];
                        if stream.read_exact(&mut body).await.is_err() {
                            break;
                        }
                        let call: Value = magi_ipc::Wire::read(&body).expect("a family call");
                        let reply = match call["call"].as_str().unwrap_or_default() {
                            "layout" => {
                                let cursors: Vec<u64> = call["args"][1]["live"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_u64)
                                    .collect();
                                let slots = items(&cursors);
                                *live.lock().expect("live") = cursors;
                                Reply::of(json!({ "id": "L-1", "slots": slots }))
                            }
                            "overflowed" => {
                                let cursors = live.lock().expect("live").clone();
                                Reply::of(json!({ "id": "L-2", "slots": tighter(&cursors) }))
                            }
                            _ => Reply::done(),
                        };
                        let bytes = reply.encode(magi_ipc::Wire::Json);
                        let _ = stream.write_u32(bytes.len() as u32).await;
                        let _ = stream.write_all(&bytes).await;
                    }
                });
            }
        });
        Self { dir }
    }

    async fn scribe(&self) -> magi_host::scribe::Held {
        let at = self.dir.join("m.sock");
        let family = Family::dial(&at).await.expect("dial");
        let scribe = Scribe::over(family, Some(at), &SessionId::new("s"));
        Arc::new(tokio::sync::Mutex::new(Some(scribe)))
    }
}

fn items(cursors: &[u64]) -> Vec<Value> {
    cursors
        .iter()
        .map(|cursor| json!({ "kind": "item", "cursor": cursor }))
        .collect()
}

fn user(id: &str, text: &str) -> Entry {
    Entry::User {
        images: Vec::new(),
        id: MessageId::new(id),
        text: text.into(),
        aside: String::new(),
    }
}

/// Refused as too long once, then answered.
fn refusing(name: &str) -> Mind {
    let refused = failed_line(
        "prompt is too long: 250000 tokens > 200000 maximum",
        "overflow",
    );
    Mind::turns(
        name,
        &[
            &[refused.as_str()],
            &[&text_line("a harness"), &stop_line()],
        ],
    )
}

async fn run(
    session: &tokio::sync::Mutex<magi_host::session::Session>,
    mind: &Mind,
    memory: &Memory,
) -> Result<(), magi_host::HostError> {
    let registry = magi_tools::Registry::new();
    let ops = magi_tools::ops::Real::new(std::env::temp_dir());
    let scribe = memory.scribe().await;
    magi_host::turn::run(session, &backend(mind), &registry, &ops, &scribe).await
}

#[tokio::test]
async fn a_tighter_layout_that_is_no_smaller_is_not_sent() {
    // Laid out differently — a summary where the prompt stood alone — and larger for it. A
    // provider that refused the request refuses this one too, so it is said rather than sent.
    let (session, _dir) = session("no-smaller");
    session
        .lock()
        .await
        .commit(user("u1", "what is magi"))
        .expect("commit");
    let memory = Memory::start("no-smaller", |live| {
        let mut slots = vec![json!({ "kind": "summary", "text": "EARLIER ".repeat(200) })];
        slots.extend(items(live));
        slots
    });
    let mind = refusing("overflow-no-smaller");
    let said = run(&session, &mind, &memory).await;
    assert_eq!(mind.asked(), 1, "the larger request was not sent");
    let why = format!("{said:?}");
    assert!(why.contains("nothing left to leave out"), "{why}");
}

#[tokio::test]
async fn a_tighter_layout_that_is_smaller_is_sent() {
    let (session, _dir) = session("smaller");
    {
        let mut held = session.lock().await;
        held.commit(user("u0", "read the part")).expect("commit");
        held.commit(Entry::Assistant {
            id: MessageId::new("a1"),
            text: String::new(),
            thinking: String::new(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            signatures: Default::default(),
            usage: Default::default(),
        })
        .expect("commit");
        held.commit(Entry::Tool {
            id: ToolCallId::new("c1"),
            name: "read".into(),
            args: "{\"path\":\"part.txt\"}".into(),
            result: Some(ToolResult {
                images: Vec::new(),
                output: "RESULT ".repeat(3_000),
                is_error: false,
                shown: None,
            }),
            thought_signature: None,
        })
        .expect("commit");
        held.commit(user("u1", "what is in it")).expect("commit");
    }
    // The result, the third entry, goes as its stub; everything else as it was.
    let memory = Memory::start("smaller", |live| {
        live.iter()
            .enumerate()
            .map(|(at, cursor)| match at {
                2 => json!({ "kind": "stub", "cursor": cursor, "text": "read part.txt" }),
                _ => json!({ "kind": "item", "cursor": cursor }),
            })
            .collect()
    });
    let mind = refusing("overflow-smaller");
    run(&session, &mind, &memory)
        .await
        .expect("the smaller request is answered");
    let asks = mind.asks();
    assert_eq!(asks.len(), 2, "refused once, then answered");
    assert!(
        asks[1].len() < asks[0].len(),
        "{} then {}",
        asks[0].len(),
        asks[1].len()
    );
    assert!(
        !asks[1].contains("RESULT RESULT"),
        "the result went as its stub"
    );
}
