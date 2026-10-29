#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{HistoryPage, UiCommand};
use serde_json::json;

#[test]
fn external_history_identity_preserves_the_exact_native_transcript_without_an_index() {
    for transcript in ["run", "run@child", "native/opaque:ü", &"x".repeat(NATIVE)] {
        let id = external(transcript).expect("identity");
        assert_eq!(native(&id).expect("decoded native identity"), transcript);
        assert!(id.len() <= ID_BYTES);
        assert_eq!(id, external(transcript).expect("stable identity"));
        let bytes: Vec<_> = id
            .strip_prefix(PREFIX)
            .expect("versioned prefix")
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex")
            })
            .collect();
        assert_eq!(String::from_utf8(bytes).expect("native UTF8"), transcript);
        assert!(
            crate::acp::prompt::parse(
                json!({"sessionId":id,"prompt":[{"type":"text","text":"ok"}]})
            )
            .is_ok()
        );
        assert!(
            crate::acp::settings::parse(
                json!({"sessionId":id,"configId":"model","value":"fake/one"})
            )
            .is_ok()
        );
    }
    assert_ne!(
        external("run").expect("run"),
        external("run@child").expect("child")
    );
    for invalid in ["", "x\ny", "x\0y", &"x".repeat(NATIVE + 1)] {
        assert!(external(invalid).is_err());
    }
    let too_long = "x".repeat(ID_BYTES + 1);
    assert!(
        crate::acp::prompt::parse(
            json!({"sessionId":too_long,"prompt":[{"type":"text","text":"ok"}]})
        )
        .is_err()
    );
    assert!(
        crate::acp::settings::parse(
            json!({"sessionId":too_long,"configId":"model","value":"fake/one"})
        )
        .is_err()
    );
}

#[test]
fn encoded_session_ids_reject_aliases_malformed_text_and_unknown_versions() {
    for invalid in [
        "magi-v1-",
        "magi-v1-1",
        "magi-v1-FF",
        "magi-v1-ff",
        "magi-v1-00",
        "magi-v1-0a",
        "magi-v1-c080",
        "magi-v1-6g",
        "magi-v2-61",
        "unversioned",
    ] {
        assert!(native(invalid).is_err(), "{invalid}");
        if invalid.starts_with(PREFIX) {
            assert!(validate_id(invalid).is_err());
            assert!(
                crate::acp::prompt::parse(
                    json!({"sessionId":invalid,"prompt":[{"type":"text","text":"ok"}]})
                )
                .is_err()
            );
            assert!(
                crate::acp::settings::parse(
                    json!({"sessionId":invalid,"configId":"model","value":"fake/one"})
                )
                .is_err()
            );
        }
    }
    assert!(validate_id("unversioned-unknown-session").is_ok());
    assert!(validate_id("line\nbreak").is_err());
    assert!(native(&format!("{PREFIX}{}", "61".repeat(NATIVE + 1))).is_err());
}

#[tokio::test]
async fn native_identity_probe_requires_the_exact_complete_boundary_and_transcript() {
    let native = SessionId::new("owner");
    let page = HistoryPage {
        session: native.clone(),
        transcript: Some("owner@child".into()),
        through: Cursor(42),
        after: Cursor(42),
        next: None,
        entries: vec![],
    };
    let mut cases = vec![(Ok(page.clone()), true), (Err("refused".to_owned()), false)];
    for field in [
        "session",
        "through",
        "after",
        "next",
        "transcript",
        "entries",
    ] {
        let mut bad = page.clone();
        match field {
            "session" => bad.session = SessionId::new("other"),
            "through" => bad.through = Cursor(41),
            "after" => bad.after = Cursor(41),
            "next" => bad.next = Some(Cursor(42)),
            "transcript" => bad.transcript = None,
            _ => bad.entries.push((
                Cursor(42),
                magi_proto::Entry::Notice {
                    text: "unexpected".into(),
                },
            )),
        }
        cases.push((Ok(bad), false));
    }
    for (answer, valid) in cases {
        let root = Scratch::new("acp-history", "identity");
        let socket = root.join("host");
        let listener = tokio::net::UnixListener::bind(&socket).expect("fixture socket");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("connection");
            let (read, write) = stream.into_split();
            let command = FrameReader::new(read)
                .read::<UiCommand>()
                .await
                .expect("probe command");
            assert!(
                matches!(command, UiCommand::ReadHistory { session, through:Cursor(42), after:Cursor(42), key:Some(key) } if session.as_str() == "owner" && key == "controller")
            );
            FrameWriter::new(write)
                .write(&answer)
                .await
                .expect("response");
        });
        let result = identity(&socket, "controller".into(), &native, Cursor(42)).await;
        assert_eq!(result.is_ok(), valid, "{result:?}");
        if valid {
            assert_eq!(
                result.expect("identity"),
                external("owner@child").expect("transcript key")
            );
        }
        server.await.expect("server");
    }
}

#[tokio::test(start_paused = true)]
async fn native_identity_probe_expires_without_waiting_for_an_unfinished_frame() {
    let root = Scratch::new("acp-history", "deadline");
    let socket = root.join("host");
    let listener = tokio::net::UnixListener::bind(&socket).expect("fixture socket");
    let server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.expect("connection");
        std::future::pending::<()>().await;
    });
    let result = identity(
        &socket,
        "controller".into(),
        &SessionId::new("owner"),
        Cursor(42),
    )
    .await;
    assert!(format!("{:#}", result.expect_err("deadline")).contains("deadline"));
    server.abort();
    let _ = server.await;
}
