//! The family's wire corpus, against the casper supplier's door: each client case is what a fake
//! `serve` answers a call with, and the door must take it as `cases.json` says. The door's call is
//! `{"call": "run", "args": [call], "key": key}`; the corpus is driven through the same exchange
//! with a `verbs` call, since a case's rows are not a tool's result. The door dials per call, so
//! the connection goes with every call.

use super::{Failed, exchange};
use crate::Uncancelled;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wire-corpus")
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

#[test]
fn every_client_case_is_taken_as_it_says_by_the_door() {
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(corpus().join("cases.json")).expect("cases"))
            .expect("parse");
    let dir = crate::family::FamilyScratch::new("corpus").expect("a family scratch");
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
        let took = exchange(&path, br#"{"call":"verbs"}"#, &Uncancelled);
        let closed = server.join().expect("the fake server");
        match expect {
            "accepted" => assert_eq!(
                took.as_ref().ok(),
                Some(&vec![serde_json::json!("x")]),
                "{file}"
            ),
            "failed" => assert!(
                matches!(&took, Err(Failed::Sent(why)) if why.starts_with("failed")),
                "{file}: {took:?}"
            ),
            "refused" => assert!(
                matches!(&took, Err(Failed::Sent(why)) if why.contains("family 2")),
                "{file}: {took:?}"
            ),
            "error_and_close" | "error_and_close_without_allocating" => {
                assert!(matches!(&took, Err(Failed::Sent(_))), "{file}: {took:?}");
                assert!(closed, "{file}: the connection was kept");
                if expect.ends_with("without_allocating") {
                    assert!(
                        matches!(&took, Err(Failed::Sent(why)) if why.contains("a frame carries")),
                        "{file}: {took:?}"
                    );
                }
            }
            other => panic!("{file}: no such expectation '{other}'"),
        }
    }
}
