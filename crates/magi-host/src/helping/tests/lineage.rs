use super::*;
use crate::{events, session::Session};
use magi_proto::{Entry, MessageId, SessionId};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn context() -> Option<magi_ipc::family::observing::Context> {
    let scope = events::context::lineage();
    magi_ipc::family::observing::Context::new(
        String::from(scope.run_id?),
        String::from(scope.agent_id?),
        scope.turn_id.map(String::from),
        scope.ask_id.map(String::from),
    )
}

fn admit(session: &mut Session) -> u64 {
    session
        .admit(crate::session::admission::Request::Opening(Entry::User {
            id: MessageId::new("prompt"),
            text: "private prompt".into(),
            images: Vec::new(),
            aside: String::new(),
        }))
        .expect("admit")
        .expect("active")
        .0
}

#[test]
fn between_captures_admission_before_release_and_does_not_borrow_a_later_turn() {
    const PROBE: &str = "MAGI_BETWEEN_LINEAGE_PROBE";
    if std::env::var_os(PROBE).is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "helping::tests::lineage::between_captures_admission_before_release_and_does_not_borrow_a_later_turn"])
            .env(PROBE, "1").output().expect("isolated probe");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), scenario())
            .await
            .expect("scenario deadline");
    });
}

async fn scenario() {
    let _receiver = events::enable_root().expect("events");
    assert!(magi_ipc::family::observing::install(context));
    let scratch = magi_model::scratch::Scratch::new("helper-lineage", "between");
    let path = scratch.join("memory.sock");
    let listener = tokio::net::UnixListener::bind(&path).expect("listen");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut heard = Vec::new();
        for verb in ["verbs", "jobs", "jobs"] {
            let n = stream.read_u32().await.expect("header");
            assert!(n <= 4096);
            let mut bytes = vec![0; n as usize];
            stream.read_exact(&mut bytes).await.expect("body");
            let request: Value = magi_ipc::Wire::read(&bytes).expect("request");
            assert_eq!(request["call"], verb);
            heard.push(request);
            let reply = serde_json::to_vec(
                &json!({"ok":true,"family":1,"n":0,"result":[],"request_context":1}),
            )
            .expect("reply");
            stream.write_u32(reply.len() as u32).await.expect("header");
            stream.write_all(&reply).await.expect("body");
        }
        heard
    });
    let mut family = magi_ipc::family::Family::dial(&path).await.expect("dial");
    family.call("verbs", vec![]).await.expect("negotiate");
    let id = SessionId::new("private-session");
    let scribe = Arc::new(tokio::sync::Mutex::new(Some(crate::scribe::Scribe::over(
        family,
        Some(path),
        &id,
    ))));
    let session = Arc::new(tokio::sync::Mutex::new(Session::recorded(id, Vec::new())));
    let mind = magi_testkit::Mind::answering("helper-lineage-unused", "unused");
    let backend = backend(&mind, Helpers::default());
    let first = admit(&mut *session.lock().await);
    let origin = session.lock().await.event_turn().expect("origin");
    let ambient = events::Id::generate().expect("ambient");
    let mut expected = Vec::new();
    for active in [true, false] {
        expected.push(if active { Some(origin) } else { None });
        let over = events::context::turn(
            Some(ambient),
            between(Arc::clone(&session), backend.clone(), Arc::clone(&scribe)),
        )
        .await;
        let later = {
            let mut held = session.lock().await;
            if active {
                assert!(held.finish(first).is_none());
            }
            let later = admit(&mut held);
            assert_ne!(held.event_turn(), Some(origin));
            later
        };
        over.send(()).expect("release");
        let tasks = session.lock().await.helpers();
        tasks
            .pause()
            .expect("pause")
            .drain(Duration::from_secs(5))
            .await
            .expect("settled");
        assert!(session.lock().await.finish(later).is_none());
    }
    let heard = server.await.expect("server");
    for (request, turn) in heard.into_iter().skip(1).zip(expected) {
        assert_eq!(request["event_context"]["turn_id"], json!(turn));
        assert_eq!(request["event_context"]["ask_id"], Value::Null);
        assert_eq!(
            request["event_context"]["run_id"],
            json!(events::process_lineage().run_id)
        );
    }
    assert!(mind.asks().is_empty());
}
