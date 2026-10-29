use super::*;
use magi_ipc::family::{Family, Reply};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, Entry, MessageId, SessionId};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod recovery;

struct Fake {
    root: Scratch,
    calls: Arc<Mutex<Vec<(usize, Value)>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Fake {
    async fn new(
        name: &str,
        answer: impl Fn(&Value) -> Option<Reply> + Send + Sync + 'static,
    ) -> Self {
        Self::paused(name, answer, None).await
    }

    async fn paused(
        name: &str,
        answer: impl Fn(&Value) -> Option<Reply> + Send + Sync + 'static,
        pause: Option<Arc<recovery::Pause>>,
    ) -> Self {
        let root = Scratch::new("mwo", name);
        let listener = tokio::net::UnixListener::bind(root.join("m.sock")).expect("bind");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        let answer = Arc::new(answer);
        let server = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            let mut connection = 0;
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.expect("accept");
                        let (seen, answer) = (Arc::clone(&seen), Arc::clone(&answer));
                        let pause = pause.clone();
                        connection += 1;
                        let connection = connection;
                        clients.spawn(async move {
                            while let Ok(n) = stream.read_u32().await {
                                assert!(n < 8192);
                                let mut body = vec![0; n as usize];
                                if stream.read_exact(&mut body).await.is_err() { break; }
                                let request: Value = magi_ipc::Wire::read(&body).expect("request");
                                seen.lock().expect("calls").push((connection, request.clone()));
                                if let Some(pause) = &pause {
                                    pause.wait(&request).await;
                                }
                                let Some(reply) = answer(&request) else { break; };
                                let body = reply.encode(magi_ipc::Wire::Json);
                                if stream.write_u32(body.len() as u32).await.is_err() || stream.write_all(&body).await.is_err() { break; }
                            }
                        });
                    }
                    _ = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Self {
            root,
            calls,
            server,
        }
    }

    async fn scribe(&self, id: &str) -> Scribe {
        let family = Family::dial(self.root.join("m.sock")).await.expect("dial");
        Scribe::over(family, None, &SessionId::new(id))
    }

    fn calls(&self) -> Vec<(usize, Value)> {
        self.calls.lock().expect("calls").clone()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn entry() -> Entry {
    Entry::User {
        id: MessageId::new("u"),
        text: "kept".into(),
        aside: String::new(),
        images: Vec::new(),
    }
}

fn compatible(request: &Value) -> Option<Reply> {
    Some(match request["call"].as_str().expect("verb") {
        "verbs" => Reply::rows(vec![
            json!({"name":"writer"}),
            json!({"name":"writer_status"}),
        ]),
        "writer_status" => Reply::of(
            json!({"revision":1,"supported":true,"owned":false,"lifetime":"connection","scope":"store-transcript"}),
        ),
        "writer" => Reply::of(json!({"revision":1,"owned":true})),
        "resume" => Reply::of(json!({"run":request["args"][0]})),
        "replay" if request["args"][1]["from"] == 0 => Reply::rows(vec![
            json!({"cursor":1,"raw":magi_journal::Record::Entry {cursor:Cursor(1),entry:entry()}}),
        ]),
        _ => Reply::done(),
    })
}

#[tokio::test]
async fn legacy_core_stays_available_but_cannot_satisfy_required_ownership() {
    let fake = Fake::new("legacy", |_| Some(Reply::done())).await;
    let mut scribe = fake.scribe("A").await;
    assert!(!scribe.reserve_writer(false).await.expect("legacy optional"));
    assert!(matches!(
        scribe.reserve_writer(true).await,
        Err(Fault::Refused(_))
    ));
    scribe
        .observe(Cursor(1), &entry(), &Default::default())
        .await
        .expect("legacy core");
    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1["call"], "verbs");
    assert_eq!(calls[1].1["call"], "observe");
}

#[tokio::test]
async fn a_reconnected_legacy_provider_is_negotiated_before_writes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let legacy = AtomicBool::new(true);
    let lost = AtomicBool::new(false);
    let fake = Fake::new("reconnect", move |request| {
        if request["call"] == "verbs" && legacy.swap(false, Ordering::SeqCst) {
            Some(Reply::done())
        } else if request["call"] == "resume" && !lost.swap(true, Ordering::SeqCst) {
            None
        } else {
            compatible(request)
        }
    })
    .await;
    let mut scribe = fake.scribe("A").await;
    assert!(!scribe.reserve_writer(false).await.expect("legacy"));
    assert!(scribe.ask_all("resume", json!({})).await.is_err());
    scribe
        .observe(Cursor(1), &entry(), &Default::default())
        .await
        .expect("new owner");
    assert!(matches!(&scribe.writer, Writer::Owned(key) if key == "A"));
    let calls = fake.calls();
    let second: Vec<_> = calls
        .iter()
        .filter(|(connection, _)| *connection == 2)
        .map(|(_, call)| call["call"].as_str().expect("verb"))
        .collect();
    assert_eq!(second, ["verbs", "writer_status", "writer", "observe"]);
}

#[tokio::test]
async fn loading_reserves_before_owner_lookup_and_replay_on_one_connection() {
    let fake = Fake::new("load", compatible).await;
    let original = fake.scribe("A").await;
    let (mut next, rows) = original.load_for("B", true).await.expect("prepared B");
    assert_eq!(rows, vec![(Cursor(1), entry())]);
    next.settle(Cursor(1), &entry(), &Default::default())
        .await
        .expect("amend restored cursor");
    let calls = fake.calls();
    let verbs: Vec<_> = calls
        .iter()
        .map(|(_, call)| call["call"].as_str().expect("verb"))
        .collect();
    assert_eq!(
        verbs,
        [
            "verbs",
            "writer_status",
            "writer",
            "resume",
            "replay",
            "replay",
            "amend"
        ]
    );
    assert!(calls.iter().all(|(connection, _)| *connection == 2));
    assert!(matches!(&next.writer, Writer::Owned(key) if key == "B"));
}

#[tokio::test]
async fn same_key_resume_keeps_the_same_connection_and_reservation() {
    let fake = Fake::new("same", compatible).await;
    let mut scribe = fake.scribe("A").await;
    assert!(scribe.reserve_writer(true).await.expect("reserve"));
    let (next, journal) = scribe.prepare_resume("A").await.expect("same key resume");
    assert!(next.is_none());
    scribe.accept_resume(&journal).expect("accept history");
    scribe
        .settle(Cursor(1), &entry(), &Default::default())
        .await
        .expect("continue");
    let calls = fake.calls();
    assert!(calls.iter().all(|(connection, _)| *connection == 1));
    assert_eq!(
        calls
            .iter()
            .filter(|(_, call)| call["call"] == "writer")
            .count(),
        1
    );
    assert_eq!(calls.last().expect("amend").1["call"], "amend");
}

#[tokio::test]
async fn failed_target_reservation_preserves_the_old_writer() {
    let fake = Fake::new("busy", |request| {
        if request["call"] == "writer" && request["args"][0] == "B" {
            Some(Reply::refused("busy"))
        } else {
            compatible(request)
        }
    })
    .await;
    let mut scribe = fake.scribe("A").await;
    scribe.reserve_writer(true).await.expect("reserve A");
    assert!(scribe.prepare_resume("B").await.is_err());
    scribe
        .observe(Cursor(1), &entry(), &Default::default())
        .await
        .expect("still A");
    assert!(matches!(&scribe.writer, Writer::Owned(key) if key == "A"));
    let calls = fake.calls();
    assert!(!calls.iter().any(|(_, call)| call["call"] == "replay"));
    assert_eq!(calls.last().expect("last").0, 1);
}

#[tokio::test]
async fn a_lost_owned_write_is_fatal_and_never_retried_on_another_connection() {
    let fake = Fake::new("lost", |request| {
        if request["call"] == "observe" {
            None
        } else {
            compatible(request)
        }
    })
    .await;
    let mut scribe = fake.scribe("A").await;
    scribe.reserve_writer(true).await.expect("reserve");
    for _ in 0..2 {
        let fault = scribe
            .observe(Cursor(1), &entry(), &Default::default())
            .await
            .expect_err("lost writer");
        assert!(matches!(fault, Fault::Failed(_)));
        assert!(fault.is_fatal());
    }
    let calls = fake.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|(_, call)| call["call"] == "observe")
            .count(),
        1
    );
    assert!(calls.iter().all(|(connection, _)| *connection == 1));
}

#[tokio::test]
async fn refused_negotiation_preserves_entries_instead_of_acknowledging_unsent_writes() {
    for refused in ["verbs", "writer_status", "writer"] {
        let fake = Fake::new("negotiation", move |request| {
            if request["call"] == refused {
                Some(Reply::refused("not available"))
            } else {
                compatible(request)
            }
        })
        .await;
        let mut scribe = fake.scribe("A").await;
        assert!(scribe.reserve_writer(false).await.is_err());
        let session = tokio::sync::Mutex::new(crate::session::Session::recorded(
            SessionId::new("A"),
            Vec::new(),
        ));
        session.lock().await.commit(entry()).expect("commit");
        let pending = session.lock().await.pending_batch();
        let mut scribe = Some(scribe);
        for _ in 0..2 {
            assert!(
                super::super::flush(&session, &mut scribe)
                    .await
                    .expect_err("negotiation failed")
                    .is_fatal()
            );
            assert_eq!(session.lock().await.pending_batch(), pending);
        }
        assert!(
            !fake
                .calls()
                .iter()
                .any(|(_, call)| call["call"] == "observe")
        );
    }
}

#[tokio::test]
async fn a_lost_owned_flush_keeps_pending_entries_before_guarded_recovery() {
    let fake = Fake::new("flush", |request| {
        if request["call"] == "observe" {
            None
        } else {
            compatible(request)
        }
    })
    .await;
    let mut scribe = fake.scribe("A").await;
    scribe.reserve_writer(true).await.expect("reserve");
    let session = tokio::sync::Mutex::new(crate::session::Session::recorded(
        SessionId::new("A"),
        Vec::new(),
    ));
    session.lock().await.commit(entry()).expect("commit");
    let pending = session.lock().await.pending_batch();
    let mut scribe = Some(scribe);
    assert!(
        super::super::flush(&session, &mut scribe)
            .await
            .expect_err("failed flush")
            .is_fatal()
    );
    assert_eq!(session.lock().await.pending_batch(), pending);
    let calls = fake.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|(_, call)| call["call"] == "observe")
            .count(),
        1
    );
    assert!(calls.iter().all(|(connection, _)| *connection == 1));
}

#[tokio::test]
async fn malformed_or_lost_acquisition_never_downgrades_to_legacy_writes() {
    for reply in [
        None,
        Some(Reply::done()),
        Some(Reply::of(json!({"revision":2,"owned":true}))),
    ] {
        let fake = Fake::new("acquire", move |request| {
            if request["call"] == "writer" {
                reply.clone()
            } else {
                compatible(request)
            }
        })
        .await;
        let mut scribe = fake.scribe("A").await;
        assert!(scribe.reserve_writer(false).await.is_err());
        assert!(
            scribe
                .observe(Cursor(1), &entry(), &Default::default())
                .await
                .is_err()
        );
        assert!(
            !fake
                .calls()
                .iter()
                .any(|(_, call)| call["call"] == "observe")
        );
    }
}
