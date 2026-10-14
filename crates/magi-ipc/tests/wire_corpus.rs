//! The family's wire corpus, against both family clients in this crate: each client case is what a
//! fake server answers a call with, and the client must take it as `cases.json` says.

use magi_ipc::family::{Fault, blocking};
use magi_model::scratch::Scratch;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wire-corpus")
}

/// Every client case: its file, what it expects, its bytes, and whether the server hangs up after.
fn cases() -> Vec<(String, String, Vec<u8>, bool)> {
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(corpus().join("cases.json")).expect("cases"))
            .expect("parse");
    cases["client"]
        .as_array()
        .expect("client cases")
        .iter()
        .map(|case| {
            let file = case["file"].as_str().expect("file").to_owned();
            let bytes = std::fs::read(corpus().join(&file)).expect("case");
            let expect = case["expect"].as_str().expect("expect").to_owned();
            (
                file,
                expect,
                bytes,
                case["eof_after"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

/// Answer the first call on `path` with `case`. What comes back is whether the client closed the
/// connection afterwards, asked only when `watch` says to and the server did not hang up itself.
fn serve(
    path: &Path,
    case: Vec<u8>,
    eof_after: bool,
    watch: bool,
) -> std::thread::JoinHandle<bool> {
    let listener = UnixListener::bind(path).expect("bind");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut head = [0_u8; 4];
        stream.read_exact(&mut head).expect("a call's head");
        let mut body = vec![0_u8; u32::from_be_bytes(head) as usize];
        stream.read_exact(&mut body).expect("a call's body");
        stream.write_all(&case).expect("the case");
        if eof_after || !watch {
            return true;
        }
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("a deadline");
        matches!(stream.read(&mut [0_u8; 1]), Ok(0))
    })
}

/// That `took` is what `expect` says, and the connection is closed where it must be.
fn judged(file: &str, expect: &str, took: &Result<Vec<serde_json::Value>, Fault>, closed: bool) {
    match expect {
        "accepted" => assert_eq!(
            took.as_ref().ok(),
            Some(&vec![serde_json::json!("x")]),
            "{file}"
        ),
        "failed" => assert!(matches!(took, Err(Fault::Failed(_))), "{file}: {took:?}"),
        "refused" => assert!(matches!(took, Err(Fault::Refused(_))), "{file}: {took:?}"),
        "error_and_close" | "error_and_close_without_allocating" => {
            assert!(
                matches!(took, Err(Fault::Unavailable(_) | Fault::Malformed(_))),
                "{file}: {took:?}"
            );
            assert!(closed, "{file}: the connection was kept");
            if expect.ends_with("without_allocating") {
                let why = took
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                assert!(why.contains("a frame carries"), "{file}: {why}");
            }
        }
        other => panic!("{file}: no such expectation '{other}'"),
    }
}

#[tokio::test]
async fn every_client_case_is_taken_as_it_says_by_the_held_client() {
    let dir = Scratch::new("magi-corpus", "async");
    for (n, (file, expect, bytes, eof_after)) in cases().into_iter().enumerate() {
        let path = dir.join(format!("{n}.sock"));
        let server = serve(&path, bytes, eof_after, expect.starts_with("error"));
        let mut family = magi_ipc::family::Family::dial(&path).await.expect("dial");
        let took = family.call("verbs", Vec::new()).await;
        let closed = server.join().expect("the fake server");
        judged(&file, &expect, &took, closed);
        drop(family);
    }
}

#[test]
fn every_client_case_is_taken_as_it_says_by_the_blocking_client() {
    let dir = Scratch::new("magi-corpus", "blocking");
    for (n, (file, expect, bytes, eof_after)) in cases().into_iter().enumerate() {
        let path = dir.join(format!("{n}.sock"));
        let server = serve(&path, bytes, eof_after, expect.starts_with("error"));
        let mut family = blocking::Family::dial(&path).expect("dial");
        let took = family.call("verbs", Vec::new());
        let closed = server.join().expect("the fake server");
        judged(&file, &expect, &took, closed);
        drop(family);
    }
}
