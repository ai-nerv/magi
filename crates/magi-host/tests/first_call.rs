//! The first call a freshly started memory layer answers is the one that opens its store, measured
//! at ten seconds on a busy machine. The calls a session cannot do without must outwait that; a
//! feature's call may give up. Against a peer that answers after the ordinary clock has run out
//! and well inside the durable one.

use magi_host::scribe::Scribe;
use magi_ipc::family::{Family, Fault};
use magi_model::scratch::Scratch;
use magi_proto::SessionId;
use std::io::{Read, Write};
use std::path::Path;

/// Past the ordinary clock, inside the durable one.
const SLOW: std::time::Duration = std::time::Duration::from_millis(2500);

/// A peer that answers one call, slowly, with `result` as its only row.
fn slow(path: &Path, result: serde_json::Value) -> std::thread::JoinHandle<()> {
    let listener = std::os::unix::net::UnixListener::bind(path).expect("bind");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut head = [0_u8; 4];
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
        if stream.read_exact(&mut body).is_err() {
            return;
        }
        std::thread::sleep(SLOW);
        let reply = serde_json::json!({
            "ok": true, "family": 1, "surface": 1, "n": 1, "result": [result]
        })
        .to_string();
        let mut framed = (reply.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(reply.as_bytes());
        let _ = stream.write_all(&framed);
    })
}

/// A scribe onto a slow peer of its own.
async fn onto(dir: &Scratch, result: serde_json::Value) -> (Scribe, std::thread::JoinHandle<()>) {
    let path = dir.join("api@s.sock");
    let served = slow(&path, result);
    let family = Family::dial(&path).await.expect("dial");
    (
        Scribe::over(family, Some(path), &SessionId::new("s")),
        served,
    )
}

#[tokio::test]
async fn resuming_outwaits_a_store_that_is_still_opening() {
    // `--resume` asks `sessions` first. Given up at two seconds, a session with a history started
    // empty on a busy machine, which is what `resume_live` kept catching in a full suite.
    let dir = Scratch::new("fc", "runs");
    let (mut scribe, served) = onto(&dir, serde_json::json!({ "id": "s-1" })).await;
    let runs = scribe
        .sessions()
        .await
        .expect("the runs, however long the store took");
    assert_eq!(runs.len(), 1, "{runs:?}");
    let _ = served.join();
}

#[tokio::test]
async fn keeping_something_outwaits_it_too() {
    // The first write to a project's store opens it, which is what `injecting_live` kept catching.
    let dir = Scratch::new("fc", "keep");
    let (mut scribe, served) = onto(&dir, serde_json::json!({ "id": "m-1" })).await;
    assert_eq!(scribe.keep("x").await.expect("kept"), "m-1");
    let _ = served.join();
}

#[tokio::test]
async fn a_features_call_still_gives_up() {
    // The control. Without it the two above pass against a fixture that is not slow at all.
    let dir = Scratch::new("fc", "lib");
    let (mut scribe, served) = onto(&dir, serde_json::json!("-- source")).await;
    let gave_up = scribe.library().await;
    assert!(matches!(gave_up, Err(Fault::Unavailable(_))), "{gave_up:?}");
    let _ = served.join();
}
