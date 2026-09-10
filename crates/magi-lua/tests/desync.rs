//! A call keeps its own answer, driven against a socket the test owns.
//!
//! The family's wire carries no request id, so a reply is matched to a call by position alone. A
//! caller that gives up on a read and keeps the connection reads the abandoned reply as the next
//! call's answer. These drive the real client libraries, through the real socket primitive, against
//! a listener that answers the first call too slowly on purpose.

use magi_lua::Engine;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// How long the client waits for a reply, and how long the first reply is withheld. Far enough
/// apart that a loaded machine cannot make the first call succeed.
const PATIENCE: Duration = Duration::from_millis(150);
const WITHHELD: Duration = Duration::from_millis(900);

/// A client library, read from the tree at run time the way the product reads it.
fn source(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A socket path this VM is allowed to dial: `magi.stream` refuses anything outside the runtime
/// directories, and keeps it short enough for `SUN_LEN`.
fn socket_path(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir();
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("mg-ds-{tag}-{}.sock", std::process::id()))
}

/// The verb a request frame names, without a JSON parser.
fn verb_of(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let Some(rest) = text.split("\"call\":\"").nth(1) else {
        return "?".to_owned();
    };
    rest.split('"').next().unwrap_or("?").to_owned()
}

/// Answer every call with the name of the verb that asked, so a reply read by the wrong call names
/// the call it really belongs to. The first answer of the whole run is withheld past the client's
/// patience; the rest are immediate.
fn answer(mut stream: UnixStream, served: &AtomicUsize) {
    loop {
        let mut head = [0_u8; 4];
        if stream.read_exact(&mut head).is_err() {
            return;
        }
        let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
        if stream.read_exact(&mut body).is_err() {
            return;
        }
        if served.fetch_add(1, Ordering::SeqCst) == 0 {
            std::thread::sleep(WITHHELD);
        }
        let reply = format!(
            "{{\"ok\":true,\"n\":1,\"result\":[\"{}\"]}}",
            verb_of(&body)
        );
        let mut frame = (reply.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(reply.as_bytes());
        if stream.write_all(&frame).is_err() || stream.flush().is_err() {
            return;
        }
    }
}

/// A listener serving each connection on its own thread, for as long as the test holds it. A
/// connection per thread because a client that dials per call has its second connection waiting
/// while the first is still being withheld.
fn listen(path: &std::path::Path) -> (std::thread::JoinHandle<()>, Arc<AtomicUsize>) {
    std::fs::remove_file(path).ok();
    let listener = UnixListener::bind(path).expect("bind");
    let served = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&served);
    let handle = std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let counted = Arc::clone(&counted);
            std::thread::spawn(move || answer(stream, &counted));
        }
    });
    (handle, served)
}

/// Two calls on one client: the first times out, the second must not be handed the first's answer.
///
/// The answer is a single string so the harvest can carry it: what each call returned, and why it
/// did not.
fn two_calls(client: &str, chunk: &str, path: &std::path::Path) -> String {
    let path = path.to_string_lossy().into_owned();
    let script = format!(
        r#"
        local chunk = assert(load({client:?}, {chunk:?}))
        local lib = chunk(magi.stream)
        local session, why = lib.connect({{ path = {path:?}, timeout_ms = {ms} }})
        if not session then
          magi.answer = "no connection: " .. tostring(why)
          return
        end
        local first, first_why = session:call("alpha")
        local second, second_why = session:call("beta")
        magi.answer = table.concat({{
          tostring(first), tostring(first_why), tostring(second), tostring(second_why),
        }}, "|")
        "#,
        ms = PATIENCE.as_millis(),
    );

    let mut engine = Engine::new();
    engine.run(&script, "desync.lua").expect("the client runs");
    engine.harvest();
    engine
        .config()
        .string("answer")
        .expect("an answer")
        .to_owned()
}

#[test]
fn a_held_connection_does_not_hand_the_next_call_the_last_answer() {
    // oslo's client keeps its handle across calls, which is the shape the rule is about.
    let path = socket_path("oslo");
    let (_serving, served) = listen(&path);

    let answer = two_calls(&source("../../config/clients/oslo.lua"), "oslo.lua", &path);
    std::fs::remove_file(&path).ok();

    let parts: Vec<&str> = answer.split('|').collect();
    assert_eq!(
        parts[0], "nil",
        "the withheld first call must not answer: {answer}"
    );
    assert_ne!(
        parts[2], "alpha",
        "the second call was handed the first call's answer: {answer}"
    );
    assert_eq!(
        parts[2], "nil",
        "a poisoned handle answers with a refusal, not a value: {answer}"
    );
    assert!(
        parts[3].contains("closed"),
        "a closed handle must say so rather than crash: {answer}"
    );
    assert!(served.load(Ordering::SeqCst) >= 1, "the listener was asked");
}

#[test]
fn a_reconnecting_client_is_in_step_on_the_next_call() {
    // magi's own client dials per call, so the abandoned reply dies with its connection. Driven the
    // same way, so the claim is measured rather than read off the source.
    let path = socket_path("magi");
    let (_serving, _served) = listen(&path);

    let answer = two_calls(&source("lua/magi.lua"), "magi.lua", &path);
    std::fs::remove_file(&path).ok();

    let parts: Vec<&str> = answer.split('|').collect();
    assert_eq!(
        parts[0], "nil",
        "the withheld first call must not answer: {answer}"
    );
    assert_ne!(
        parts[2], "alpha",
        "the second call was handed the first call's answer: {answer}"
    );
    assert_eq!(
        parts[2], "beta",
        "the second call gets its own answer: {answer}"
    );
}
