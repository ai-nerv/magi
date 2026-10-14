//! The family's wire corpus, against the client library this program serves (`magi.lua`), in
//! magi's own VM over `magi.stream`: each client case is what a fake server answers a call with,
//! and the client must take it as `cases.json` says. The library dials per call, so every call
//! closes its connection whatever came back.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_lua::Engine;
use magi_lua::client::CLIENT;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wire-corpus")
}

/// Answer the first call on `path` with `case`, past any connection that only probes. What comes
/// back is whether the client closed that connection, asked only when `watch` says to and the
/// server did not hang up itself.
fn serve(
    path: &Path,
    case: Vec<u8>,
    eof_after: bool,
    watch: bool,
) -> std::thread::JoinHandle<bool> {
    let listener = UnixListener::bind(path).expect("bind");
    std::thread::spawn(move || {
        loop {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut head = [0_u8; 4];
            if stream.read_exact(&mut head).is_err() {
                continue;
            }
            let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
            stream.read_exact(&mut body).expect("a call's body");
            stream.write_all(&case).expect("the case");
            if eof_after || !watch {
                return true;
            }
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("a deadline");
            return matches!(stream.read(&mut [0_u8; 1]), Ok(0));
        }
    })
}

/// What the client made of the reply to one call, as `value|why`.
fn took(path: &Path) -> String {
    let probe = format!(
        "local client = assert(load({CLIENT:?}, 'magi.lua'))(magi.stream) \
         local s = assert(client.connect({{ path = {path:?}, timeout_ms = 1000 }})) \
         local value, why = s:call('verbs') \
         magi.answer = tostring(value) .. '|' .. tostring(why)",
        path = path.to_string_lossy()
    );
    let mut engine = Engine::new();
    engine
        .run(&probe, "corpus.lua")
        .expect("the client must not raise");
    engine.harvest();
    engine
        .config()
        .string("answer")
        .expect("an answer")
        .to_owned()
}

#[test]
fn every_client_case_is_taken_as_it_says_by_the_library() {
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(corpus().join("cases.json")).expect("cases"))
            .expect("parse");
    let dir = magi_tools::family::FamilyScratch::new("corpus").expect("a family scratch");
    for (n, case) in cases["client"]
        .as_array()
        .expect("client cases")
        .iter()
        .enumerate()
    {
        let file = case["file"].as_str().expect("file");
        let expect = case["expect"].as_str().expect("expect");
        let bytes = std::fs::read(corpus().join(file)).expect("case");
        let path = dir.join(format!("{n}.sock"));
        let server = serve(
            &path,
            bytes,
            case["eof_after"].as_bool().unwrap_or(false),
            expect.starts_with("error"),
        );
        let said = took(&path);
        let closed = server.join().expect("the fake server");
        let (value, why) = said.split_once('|').expect("value|why");
        match expect {
            "accepted" => assert_eq!(value, "x", "{file}: {said}"),
            "failed" => assert_eq!((value, why), ("nil", "no"), "{file}: {said}"),
            "refused" => {
                assert_eq!(value, "nil", "{file}: {said}");
                assert!(why.contains("family 2"), "{file}: {said}");
            }
            "error_and_close" | "error_and_close_without_allocating" => {
                assert_eq!(value, "nil", "{file}: {said}");
                assert!(closed, "{file}: the connection was kept");
                if expect.ends_with("without_allocating") {
                    assert!(why.contains("a frame carries"), "{file}: {said}");
                }
            }
            other => panic!("{file}: no such expectation '{other}'"),
        }
    }
}
