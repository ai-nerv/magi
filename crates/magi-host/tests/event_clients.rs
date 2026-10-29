#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{events::*, session::Session};
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, Entry, HarnessEvent, SessionId, UiCommand};
use magi_testkit::Scratch;
use std::{
    collections::BTreeMap,
    os::unix::fs::DirBuilderExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tokio::net::UnixStream;

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut events = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        assert!(bytes.len() <= MAX_EVENT_BYTES);
        let text = std::str::from_utf8(&bytes).expect("JSON");
        for secret in ["private", "HOME", "PATH"] {
            assert!(!text.contains(secret));
        }
        events.push(decode(&bytes).expect("strict event"));
    }
    events
}

async fn collect(receiver: &Receiver, count: usize) -> Vec<Event> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut events = vec![];
        loop {
            events.extend(drain(receiver));
            if events.len() >= count {
                assert_eq!(events.len(), count);
                return events;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("event deadline")
}

async fn inspect(stream: UnixStream) {
    let (read, write) = stream.into_split();
    FrameWriter::new(write)
        .write(&UiCommand::Inspect)
        .await
        .expect("inspect");
    let _: magi_proto::monitoring::AgentDetails =
        tokio::time::timeout(Duration::from_secs(3), FrameReader::new(read).read())
            .await
            .expect("reply deadline")
            .expect("details");
}

async fn attach(
    stream: UnixStream,
    key: Option<String>,
    latest: bool,
    history: bool,
    watcher: bool,
) {
    let (read, write) = stream.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    let command = if latest {
        UiCommand::AttachLatest { draws: false, key }
    } else {
        UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
            key,
        }
    };
    writer.write(&command).await.expect("attach");
    let snapshot: HarnessEvent = tokio::time::timeout(Duration::from_secs(3), reader.read())
        .await
        .expect("snapshot deadline")
        .expect("snapshot");
    let HarnessEvent::SessionSnapshot { entries, .. } = snapshot else {
        panic!("snapshot")
    };
    assert_eq!(entries.len(), usize::from(latest && history));
    if latest && history {
        assert!(matches!(&entries[0], Entry::User { text, .. } if text == "private-history"));
    }
    if history && !latest {
        let replay: HarnessEvent = tokio::time::timeout(Duration::from_secs(3), reader.read())
            .await
            .expect("replay deadline")
            .expect("replay");
        assert!(
            matches!(replay, HarnessEvent::UserMessage { text, .. } if text == "private-history")
        );
    }
    if watcher {
        writer
            .write(&UiCommand::SubmitTracked {
                request: 42,
                text: "private-forbidden".into(),
                aside: String::new(),
                images: vec![],
            })
            .await
            .expect("watcher command");
        let refused: HarnessEvent = tokio::time::timeout(Duration::from_secs(3), reader.read())
            .await
            .expect("refusal deadline")
            .expect("refusal");
        assert!(matches!(
            refused,
            HarnessEvent::PromptRefused { request: 42, .. }
        ));
    }
    writer.write(&UiCommand::Detach).await.expect("detach");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker() {
    let Ok(mode) = std::env::var("MAGI_EVENT_CLIENT_WORKER") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("HOME").expect("private HOME"));
    let socket = root.join("s");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let session = Session::recorded(
        SessionId::new("private-session"),
        if mode == "reconnect" {
            vec![Entry::User {
                id: magi_proto::MessageId::new("private-old"),
                text: "private-history".into(),
                aside: String::new(),
                images: vec![],
            }]
        } else {
            vec![]
        },
    );
    let receiver = if mode == "disabled" {
        None
    } else {
        Some(enable_local().expect("activation"))
    };
    if mode == "full" {
        for _ in 0..QUEUED_EVENTS {
            assert_eq!(
                emit(|| (
                    Correlation::default(),
                    Fact::Client {
                        outcome: ClientOutcome::Connected
                    }
                )),
                Emitted::Queued
            );
        }
    }
    let receiver = if mode == "closed" {
        drop(receiver);
        None
    } else {
        receiver
    };
    let host = tokio::spawn(magi_host::serve(listener, session, None));
    let mut seen = vec![];
    let stream = magi_ipc::connect(&socket).await.expect("connect");
    match mode.as_str() {
        "inspect" | "disabled" | "full" | "closed" => inspect(stream).await,
        "attach" => attach(stream, magi_ipc::read_key(&socket), true, false, false).await,
        "watcher" => attach(stream, None, true, false, true).await,
        "reconnect" => {
            attach(stream, magi_ipc::read_key(&socket), true, true, false).await;
            seen.extend(collect(receiver.as_ref().expect("receiver"), 2).await);
            let stream = magi_ipc::connect(&socket).await.expect("reconnect");
            attach(stream, magi_ipc::read_key(&socket), false, true, false).await;
        }
        "eof" => drop(stream),
        "invalid" | "malformed" => {
            let (read, write) = stream.into_split();
            if mode == "invalid" {
                FrameWriter::new(write)
                    .write(&UiCommand::Detach)
                    .await
                    .expect("invalid first command");
            } else {
                FrameWriter::new(write)
                    .write(&"private-invalid-frame")
                    .await
                    .expect("malformed frame");
            }
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(3),
                    FrameReader::new(read).read::<HarnessEvent>()
                )
                .await
                .expect("closure deadline")
                .is_err()
            );
        }
        "timeout" => {
            let began = Instant::now();
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(35),
                    FrameReader::new(stream).read::<HarnessEvent>()
                )
                .await
                .expect("native attach timeout")
                .is_err()
            );
            assert!(began.elapsed() >= Duration::from_secs(29));
        }
        "history" | "history_denied" => {
            let (read, write) = stream.into_split();
            FrameWriter::new(write)
                .write(&UiCommand::ReadHistory {
                    session: SessionId::new("private-session"),
                    through: Cursor::ZERO,
                    after: Cursor::ZERO,
                    key: if mode == "history" {
                        magi_ipc::read_key(&socket)
                    } else {
                        None
                    },
                })
                .await
                .expect("history request");
            let reply: Result<magi_proto::HistoryPage, String> =
                tokio::time::timeout(Duration::from_secs(3), FrameReader::new(read).read())
                    .await
                    .expect("history deadline")
                    .expect("history response");
            assert_eq!(reply.is_ok(), mode == "history");
        }
        "review_denied" => {
            let (read, write) = stream.into_split();
            FrameWriter::new(write)
                .write(&UiCommand::Review {
                    tool: "private-tool".into(),
                    action: magi_proto::permit::Action::Read {
                        path: "/private/path".into(),
                    },
                    key: None,
                })
                .await
                .expect("review");
            let reply: magi_proto::permit::Decision =
                tokio::time::timeout(Duration::from_secs(3), FrameReader::new(read).read())
                    .await
                    .expect("review deadline")
                    .expect("decision");
            assert_eq!(reply, magi_proto::permit::Decision::Deny);
        }
        "busy" => {
            let mut clients = vec![stream];
            for _ in 1..64 {
                clients.push(magi_ipc::connect(&socket).await.expect("client"));
            }
            seen.extend(collect(receiver.as_ref().expect("receiver"), 64).await);
            assert!(seen.iter().all(|event| event.fact
                == Fact::Client {
                    outcome: ClientOutcome::Connected
                }));
            let rejected = magi_ipc::connect(&socket)
                .await
                .expect("overflow connection");
            let refused: HarnessEvent =
                tokio::time::timeout(Duration::from_secs(3), FrameReader::new(rejected).read())
                    .await
                    .expect("busy deadline")
                    .expect("busy response");
            assert!(matches!(refused, HarnessEvent::Refused { .. }));
            seen.extend(collect(receiver.as_ref().expect("receiver"), 1).await);
            drop(clients);
        }
        _ => panic!("unexpected mode"),
    }
    if mode == "disabled" {
        let receiver = enable_local().expect("late activation");
        assert!(drain(&receiver).is_empty());
        assert_eq!(loss(), Loss::default());
    } else if mode == "full" || mode == "closed" {
        tokio::time::timeout(Duration::from_secs(3), async {
            while (if mode == "full" {
                loss().full
            } else {
                loss().closed
            }) < 2
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("closed client loss");
        if mode == "full" {
            assert_eq!(loss().full, 2);
            assert_eq!(
                drain(receiver.as_ref().expect("receiver")).len(),
                QUEUED_EVENTS
            );
        } else {
            assert_eq!(loss().closed, 2);
        }
    } else {
        seen.extend(
            collect(
                receiver.as_ref().expect("receiver"),
                if mode == "busy" { 64 } else { 2 },
            )
            .await,
        );
        let mut clients: BTreeMap<String, Vec<ClientOutcome>> = BTreeMap::new();
        for event in seen {
            let id = event.correlation.request_id.expect("client ID");
            assert_eq!(
                event.correlation,
                Correlation {
                    request_id: Some(id),
                    ..Correlation::default()
                }
            );
            let Fact::Client { outcome } = event.fact else {
                panic!("unexpected non-client event")
            };
            clients.entry(id.into()).or_default().push(outcome);
        }
        assert_eq!(
            clients.len(),
            match mode.as_str() {
                "busy" => 65,
                "reconnect" => 2,
                _ => 1,
            }
        );
        let mut refused = 0;
        for outcomes in clients.values() {
            if outcomes == &[ClientOutcome::Refused] {
                refused += 1;
            } else {
                assert_eq!(
                    outcomes,
                    &[ClientOutcome::Connected, ClientOutcome::Disconnected]
                );
            }
        }
        assert_eq!(refused, usize::from(mode == "busy"));
        assert_eq!(loss(), Loss::default());
    }
    host.abort();
    let _ = host.await;
}

#[test]
fn native_clients_preserve_transport_admission_and_command_authority() {
    for mode in [
        "inspect",
        "attach",
        "watcher",
        "reconnect",
        "eof",
        "invalid",
        "malformed",
        "timeout",
        "history",
        "history_denied",
        "review_denied",
        "busy",
        "disabled",
        "full",
        "closed",
    ] {
        let root = Scratch::new("event-clients", mode);
        for dir in ["home", "config", "cache", "data", "run"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(dir))
                .expect("private directory");
        }
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "worker", "--nocapture"])
            .env_clear()
            .env("MAGI_EVENT_CLIENT_WORKER", mode)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("TMPDIR", &*root)
            .env("PATH", std::env::var_os("PATH").expect("test PATH"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("isolated worker");
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = child.try_wait().expect("worker status") {
                assert!(status.success(), "scenario {mode}: {status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill worker");
                child.wait().expect("reap worker");
                panic!("scenario {mode} timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
