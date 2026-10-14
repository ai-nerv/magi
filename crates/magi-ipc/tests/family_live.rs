//! The family client against a balthasar of each test's own, skipped when balthasar is not
//! installed. Never the one whoever runs the suite has running: `observe` below writes into it.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_ipc::family::{Family, Fault};
use magi_model::scratch::Scratch;
use magi_testkit::memory::Serving;

/// What a test holds while it runs: the balthasar it started, then the directory it started in.
type Held = (Serving, Scratch);

/// A connection to a balthasar this test started. `None` when there is none to start, which
/// `MAGI_REQUIRE_LIVE=1` turns into a failure.
async fn dial(name: &str) -> Option<(Family, Held)> {
    let dir = Scratch::new("fl", name);
    let instance = format!("f{}-{name}", std::process::id());
    let Some(serving) = Serving::start(&dir, &instance).await else {
        assert!(
            !std::env::var("MAGI_REQUIRE_LIVE").is_ok_and(|v| v == "1"),
            "MAGI_REQUIRE_LIVE=1 and there is no balthasar to start"
        );
        eprintln!("skipping: no balthasar is installed");
        return None;
    };
    let family = Family::dial(serving.socket())
        .await
        .expect("dial the balthasar that just answered");
    Some((family, (serving, dir)))
}

#[tokio::test]
async fn a_running_balthasar_lists_its_verbs() {
    let Some((mut family, _held)) = dial("verbs").await else {
        return;
    };
    let values = family.call("verbs", Vec::new()).await.expect("verbs");
    let listed = format!("{values:?}");
    for verb in ["observe", "amend", "replay", "resume", "sessions"] {
        assert!(listed.contains(verb), "{verb} missing from {listed}");
    }
}

#[tokio::test]
async fn one_connection_serves_many_calls() {
    let Some((mut family, _held)) = dial("many").await else {
        return;
    };
    for _ in 0..3 {
        family.call("verbs", Vec::new()).await.expect("verbs again");
    }
}

#[tokio::test]
async fn a_verb_that_does_not_exist_is_refused_rather_than_disconnecting() {
    let Some((mut family, _held)) = dial("unknown").await else {
        return;
    };
    let answer = family.call("no_such_verb_at_all", Vec::new()).await;
    assert!(
        matches!(answer, Err(Fault::Refused(_))),
        "expected a refusal, got {answer:?}"
    );
    family
        .call("verbs", Vec::new())
        .await
        .expect("the connection survives a refusal");
}

#[tokio::test]
async fn a_turn_can_be_observed_and_replayed() {
    let Some((mut family, _held)) = dial("observe").await else {
        return;
    };
    let session = format!("magi-family-{}", std::process::id());
    let turn = serde_json::json!({
        "cursor": 1,
        "role": "user",
        "kind": "prose",
        "text": "does the wire hold",
        "raw": { "record": "entry", "cursor": 1 },
    });

    family
        .call(
            "observe",
            vec![serde_json::Value::String(session.clone()), turn],
        )
        .await
        .expect("observe");

    let back = family
        .call("replay", vec![serde_json::Value::String(session)])
        .await
        .expect("replay");
    assert!(
        format!("{back:?}").contains("does the wire hold"),
        "what went in did not come back: {back:?}"
    );
}

#[tokio::test]
async fn a_real_balthasar_answers_cbor_in_cbor() {
    // The two halves of the family agreeing, over a real socket, in both encodings.
    let Some((open, _held)) = dial("cbor").await else {
        return;
    };
    let mut cbor = open.speaking(magi_ipc::Wire::Cbor);
    let verbs = cbor.call("verbs", Vec::new()).await;
    assert!(
        verbs.is_ok(),
        "a balthasar asked in cbor must answer: {verbs:?}"
    );
    let verbs = verbs.expect("verbs");
    assert!(!verbs.is_empty(), "and say something: {verbs:?}");

    // The same connection, still in CBOR, so this is not one lucky frame.
    let again = cbor.call("verbs", Vec::new()).await;
    assert!(again.is_ok(), "and again on the same connection: {again:?}");
}

#[tokio::test]
async fn the_same_question_gets_the_same_answer_in_either_encoding() {
    // One connection asked twice: a balthasar accepts them one at a time, so dialling again while
    // holding one open waits for itself.
    let Some((mut open, _held)) = dial("same").await else {
        return;
    };
    let in_json: Vec<serde_json::Value> = open.call("verbs", Vec::new()).await.expect("json");

    let mut open = open.speaking(magi_ipc::Wire::Cbor);
    let in_cbor: Vec<serde_json::Value> = open.call("verbs", Vec::new()).await.expect("cbor");

    assert_eq!(in_json, in_cbor, "one shape, two encodings");
    assert!(!in_json.is_empty(), "and it said something: {in_json:?}");
}

/// One call by hand from another connection of this user's, carrying `key` or none.
fn by_hand(socket: &std::path::Path, call: &serde_json::Value) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("a deadline");
    let body = serde_json::to_vec(call).expect("encode");
    let mut framed = (body.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&body);
    stream.write_all(&framed).expect("write");
    let mut head = [0_u8; 4];
    stream.read_exact(&mut head).expect("a reply, not a close");
    let mut reply = vec![0_u8; u32::from_be_bytes(head) as usize];
    stream.read_exact(&mut reply).expect("reply");
    serde_json::from_slice(&reply).expect("decode")
}

#[tokio::test]
async fn a_second_peer_without_the_key_cannot_observe_into_the_session() {
    let Some((mut family, held)) = dial("peer").await else {
        return;
    };
    let session = format!("magi-peer-{}", std::process::id());
    let turn = |cursor: u64, text: &str| serde_json::json!({ "cursor": cursor, "role": "user", "kind": "prose", "text": text, "raw": {} });
    family
        .call("observe", vec![serde_json::json!(session), turn(1, "ours")])
        .await
        .expect("the session's own call, which carries the key");

    let wrong = "0".repeat(64);
    let mut refused = 0;
    for key in [None, Some(wrong.as_str())] {
        let mut call = serde_json::json!({
            "call": "observe", "args": [session, turn(2, "planted")],
        });
        if let Some(key) = key {
            call["key"] = serde_json::json!(key);
        }
        let told = by_hand(held.0.socket(), &call);
        if told["ok"] == true {
            eprintln!("skipping: this balthasar predates the session key");
            return;
        }
        assert!(
            told["error"].as_str().unwrap_or_default().contains("key"),
            "{key:?}: {told}"
        );
        refused += 1;
    }
    assert_eq!(refused, 2);

    let back = family
        .call("replay", vec![serde_json::json!(session)])
        .await
        .expect("replay");
    assert_eq!(back.len(), 1, "only what the session said: {back:?}");
}

/// `balthasar serve` started by hand in `dir`, handed no key, and the socket it bound once it has
/// published a key other than `before`; `None` when it never does.
fn by_hand_serve(
    dir: &std::path::Path,
    before: Option<&str>,
) -> Option<(std::process::Child, std::path::PathBuf)> {
    let child = std::process::Command::new("balthasar")
        .args(["serve", "--instance", "again", "--scope", "project"])
        .current_dir(dir)
        .env("XDG_RUNTIME_DIR", dir.join("r"))
        .env("HOME", dir.join("h"))
        .env("XDG_CONFIG_HOME", dir.join("c"))
        .env("XDG_DATA_HOME", dir.join("d"))
        .env_remove("MAGI_API_SOCKET")
        .env_remove("MAGI_MEMORY_INSTANCE")
        .env_remove("MAGI_BALTHASAR_INSTANCE")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let socket = dir.join("r/memory/api@again.sock");
    let published = magi_ipc::key_path(&socket);
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut bound = false;
    while std::time::Instant::now() < deadline {
        let said = std::fs::read_to_string(&published).ok();
        if said
            .as_deref()
            .is_some_and(|key| Some(key) != before && key.ends_with('\n'))
        {
            return Some((child, socket));
        }
        // Answering and publishing nothing a second later is a build from before published keys.
        if !bound && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            bound = true;
            deadline = deadline.min(std::time::Instant::now() + std::time::Duration::from_secs(1));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut child = child;
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[tokio::test]
async fn a_memory_layer_started_again_by_hand_is_written_to_with_the_key_it_published() {
    let dir = Scratch::new("fl", "again");
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
        .create(dir.join("r"))
        .expect("a private runtime directory");
    let Some((mut first, socket)) = by_hand_serve(&dir, None) else {
        eprintln!("skipping: no balthasar here publishes its own key");
        return;
    };
    let turn = |cursor: u64, text: &str| serde_json::json!({ "cursor": cursor, "role": "user", "kind": "prose", "text": text, "raw": {} });
    let mut family = Family::dial(&socket).await.expect("dial");
    family
        .call_within(
            "observe",
            vec![serde_json::json!("s"), turn(1, "before")],
            magi_ipc::family::DURABLE,
        )
        .await
        .expect("written with the key it published");

    let _ = first.kill();
    let _ = first.wait();
    let left = std::fs::read_to_string(magi_ipc::key_path(&socket)).ok();
    let (mut second, _) = by_hand_serve(&dir, left.as_deref()).expect("started again");
    let gone = family.call("verbs", Vec::new()).await;
    assert!(
        matches!(gone, Err(Fault::Unavailable(_))),
        "the connection to the first went with it: {gone:?}"
    );
    family
        .call_within(
            "observe",
            vec![serde_json::json!("s"), turn(2, "after")],
            magi_ipc::family::DURABLE,
        )
        .await
        .expect("written with the key it published this time");

    let back = by_hand(
        &socket,
        &serde_json::json!({ "call": "replay", "args": ["s"] }),
    );
    assert_eq!(back["n"], 2, "a read needs no key, and both landed: {back}");
    let _ = second.kill();
    let _ = second.wait();
}
