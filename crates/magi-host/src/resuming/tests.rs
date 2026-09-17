use super::*;
use magi_ipc::family::{Family, Reply};
use magi_model::scratch::Scratch;
use magi_proto::{Cursor, Entry, MessageId, SessionId};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod durable;

struct Peer {
    dir: Scratch,
    server: tokio::task::JoinHandle<()>,
    calls: Arc<std::sync::Mutex<Vec<Value>>>,
}

impl Peer {
    async fn new<F, Fut>(name: &str, reply: F) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Reply> + Send + 'static,
    {
        let dir = Scratch::new("magi-resume-peer", name);
        let listener = tokio::net::UnixListener::bind(dir.join("m.sock")).expect("bind fixture");
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        let reply = Arc::new(reply);
        let server = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((mut stream, _)) = accepted else { return; };
                        let reply = Arc::clone(&reply);
                        let seen = Arc::clone(&seen);
                        clients.spawn(async move {
                            while let Ok(length) = stream.read_u32().await {
                                let mut body = vec![0; length as usize];
                                if stream.read_exact(&mut body).await.is_err() { return; }
                                let request: Value = magi_ipc::Wire::read(&body).expect("family request");
                                seen.lock().expect("calls").push(request.clone());
                                let bytes = reply(request).await.encode(magi_ipc::Wire::Json);
                                if stream.write_u32(bytes.len() as u32).await.is_err() || stream.write_all(&bytes).await.is_err() { return; }
                            }
                        });
                    }
                    _ = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Self { dir, server, calls }
    }

    async fn scribe(&self) -> scribe::Held {
        let path = self.dir.join("m.sock");
        let family = Family::dial(&path).await.expect("dial fixture");
        Arc::new(Mutex::new(Some(scribe::Scribe::over(
            family,
            Some(path),
            &SessionId::new("A"),
        ))))
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn user(text: &str) -> Entry {
    Entry::User {
        id: MessageId::new(text),
        text: text.into(),
        aside: String::new(),
    }
}

fn session() -> Arc<Mutex<Session>> {
    Arc::new(Mutex::new(Session::recorded(
        SessionId::new("A"),
        vec![user("original A")],
    )))
}

fn candidate(request: &Value) -> Reply {
    match request["call"].as_str() {
        Some("resume") => Reply::rows(vec![json!({"run":"B"})]),
        Some("replay") if request["args"][1]["from"] == 0 => {
            let raw = magi_journal::Record::Entry {
                cursor: Cursor(7),
                entry: user("original B"),
            };
            Reply::rows(vec![json!({"cursor":7,"raw":raw})])
        }
        _ => Reply::done(),
    }
}

#[tokio::test]
async fn malformed_replay_cursors_refuse_resume_without_switching() {
    for cursors in [vec![0], vec![u64::MAX], vec![7, 7], vec![13, 7], vec![8]] {
        let peer = Peer::new("invalid-cursor", move |request| {
            std::future::ready(if request["call"] == "replay" {
                Reply::rows(
                    cursors
                        .iter()
                        .map(|cursor| {
                            let raw = magi_journal::Record::Entry {
                                cursor: Cursor(if *cursor == 8 { 7 } else { *cursor }),
                                entry: user("B"),
                            };
                            json!({"cursor":cursor, "raw":raw})
                        })
                        .collect(),
                )
            } else {
                candidate(&request)
            })
        })
        .await;
        let session = session();
        let scribe = peer.scribe().await;
        let why = resume(&session, &RwLock::new(None), &scribe, "B")
            .await
            .expect_err("invalid replay");
        assert!(why.contains("cursor"), "{why}");
        assert_eq!(session.lock().await.id().as_str(), "A");
        assert!(!session.lock().await.busy());
    }
}

#[tokio::test]
async fn unchanged_nonpaging_replay_is_accepted_without_duplication() {
    let peer = Peer::new("nonpaging", |mut request| {
        if request["call"] == "replay" {
            request["args"][1]["from"] = json!(0);
        }
        std::future::ready(candidate(&request))
    })
    .await;
    let session = session();
    resume(&session, &RwLock::new(None), &peer.scribe().await, "B")
        .await
        .expect("nonpaging resume");
    assert_eq!(session.lock().await.entries(), &[user("original B")]);
}

#[tokio::test]
async fn an_optional_model_extension_is_not_required_for_resume() {
    let peer = Peer::new("without-model", |request| {
        std::future::ready(if request["call"] == "model" {
            Reply::refused("no model extension")
        } else {
            candidate(&request)
        })
    })
    .await;
    let session = session();
    session.lock().await.set_model(Some(magi_proto::ModelInfo {
        name: "fake/one".into(),
        context_window: 1000,
    }));
    resume(&session, &RwLock::new(None), &peer.scribe().await, "B")
        .await
        .expect("base role supports resume");
    assert_eq!(session.lock().await.id().as_str(), "B");
}

#[tokio::test]
async fn pending_storage_failure_leaves_a_selected_and_every_unsent_row_pending() {
    let peer = Peer::new("write-failure", |request| {
        std::future::ready(if request["call"] == "observe" {
            Reply::refused("synthetic write failure")
        } else {
            candidate(&request)
        })
    })
    .await;
    let session = session();
    session
        .lock()
        .await
        .commit(user("pending A"))
        .expect("pending entry");
    let scribe = peer.scribe().await;
    let why = resume(&session, &RwLock::new(None), &scribe, "B")
        .await
        .expect_err("failed flush refuses resume");
    assert!(why.contains("write failure"));
    let held = session.lock().await;
    assert_eq!(held.id().as_str(), "A");
    assert_eq!(held.pending_batch(), vec![(Cursor(2), user("pending A"))]);
    assert!(!held.busy());
    assert!(held.helpers().pause().is_ok());
    assert!(
        peer.calls
            .lock()
            .expect("calls")
            .iter()
            .all(|r| r["args"][0] == "A")
    );
}

#[tokio::test]
async fn empty_or_refused_replay_preserves_the_original_binding_and_subscriptions() {
    for empty in [true, false] {
        let peer = Peer::new(if empty { "empty" } else { "refused" }, move |request| {
            std::future::ready(if request["call"] == "replay" {
                if empty {
                    Reply::done()
                } else {
                    Reply::refused("synthetic replay failure")
                }
            } else {
                candidate(&request)
            })
        })
        .await;
        let session = session();
        let mut events = session.lock().await.subscribe();
        let scribe = peer.scribe().await;
        assert!(
            resume(&session, &RwLock::new(None), &scribe, "B")
                .await
                .is_err()
        );
        assert_eq!(session.lock().await.id().as_str(), "A");
        assert!(
            events.try_recv().is_err(),
            "no replacement snapshot on failure"
        );
        session
            .lock()
            .await
            .commit(user("still A"))
            .expect("continue A");
        scribe::flush(&session, &mut *scribe.lock().await)
            .await
            .expect("flush A");
        let calls = peer.calls.lock().expect("calls");
        let write = calls
            .iter()
            .find(|r| r["call"] == "observe")
            .expect("write");
        assert_eq!(write["args"][0], "A");
        assert_eq!(write["args"][1]["run"], "A");
    }
}

#[tokio::test]
async fn old_helpers_settle_before_rebinding_and_cannot_spawn_into_b() {
    let peer = Peer::new("helpers", |request| std::future::ready(candidate(&request))).await;
    let session = session();
    let scribe = peer.scribe().await;
    let old = session.lock().await.helpers();
    let (release, wait) = tokio::sync::oneshot::channel();
    let old_scribe = Arc::clone(&scribe);
    old.spawn(async move {
        wait.await.map_err(|why| why.to_string())?;
        old_scribe
            .lock()
            .await
            .as_mut()
            .expect("scribe")
            .job_done(json!({"id":"A-helper"}))
            .await
            .map_err(|why| why.to_string())
    })
    .expect("old helper");
    let worker = Arc::new(RwLock::new(None));
    let (switching, open, jobs) = (
        Arc::clone(&session),
        Arc::clone(&scribe),
        Arc::clone(&worker),
    );
    let switched = tokio::spawn(async move { resume(&switching, &jobs, &open, "B").await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !session.lock().await.busy() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("transition reserved");
    assert_eq!(session.lock().await.id().as_str(), "A");
    assert!(!switched.is_finished());
    assert!(old.spawn(async { Ok(()) }).is_err());
    release.send(()).expect("release old helper");
    switched.await.expect("resume task").expect("resume B");
    assert_eq!(session.lock().await.id().as_str(), "B");
    assert!(old.spawn(async { Ok(()) }).is_err());
    let calls = peer.calls.lock().expect("calls");
    let done = calls
        .iter()
        .position(|r| r["call"] == "job_done")
        .expect("helper completion");
    let read = calls
        .iter()
        .position(|r| r["call"] == "replay")
        .expect("B preparation");
    assert!(done < read);
    assert_eq!(calls[done]["args"][0], "A");
}

#[tokio::test]
async fn cancelling_a_flush_keeps_pending_entries_until_a_later_acknowledgment() {
    let (started, mut calls) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let gate = Arc::clone(&release);
    let peer = Peer::new("cancel-flush", move |request| {
        let gate = Arc::clone(&gate);
        let started = started.clone();
        async move {
            started.send(request).expect("request observed");
            let _permit = gate.acquire().await.expect("release reply");
            Reply::done()
        }
    })
    .await;
    let session = session();
    session
        .lock()
        .await
        .commit(user("pending A"))
        .expect("pending entry");
    let scribe = peer.scribe().await;
    let (writing, open) = (Arc::clone(&session), Arc::clone(&scribe));
    let flush = tokio::spawn(async move { scribe::flush(&writing, &mut *open.lock().await).await });
    calls.recv().await.expect("write reached store");
    flush.abort();
    assert!(flush.await.expect_err("cancelled flush").is_cancelled());
    assert_eq!(
        session.lock().await.pending_batch(),
        vec![(Cursor(2), user("pending A"))]
    );
    release.add_permits(1);
    scribe::flush(&session, &mut *scribe.lock().await)
        .await
        .expect("retry flush");
    assert!(!session.lock().await.has_pending());
}
