#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{events::*, session::Session, turn::Backend};
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, Entry, HarnessEvent, SessionId, UiCommand};
use magi_testkit::{Mind, Scratch};
use std::{
    collections::BTreeMap,
    os::unix::fs::DirBuilderExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

type Reader = FrameReader<tokio::net::unix::OwnedReadHalf>;

async fn until(reader: &mut Reader, predicate: impl Fn(&HarnessEvent) -> bool) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = reader.read().await.expect("native event");
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("native event deadline")
}

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut events = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        assert!(bytes.len() <= MAX_EVENT_BYTES);
        let text = std::str::from_utf8(&bytes).expect("JSON");
        for secret in ["private", "fake/", "HOME", "PATH"] {
            assert!(!text.contains(secret));
        }
        events.push(decode(&bytes).expect("strict event"));
    }
    events
}

fn backend(mind: &Mind, root: &std::path::Path) -> Backend {
    Backend {
        tools: vec![],
        clients: vec![],
        tooling: Default::default(),
        cwd: root.into(),
        model: "fake/private-model".into(),
        mind: mind.program().display().to_string(),
        wants: Default::default(),
        context_window: Some(200_000),
        max_output: None,
        system: None,
        confine: false,
        isolate: false,
        grants: vec![],
        environ: [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_RUNTIME_DIR",
            "PATH",
        ]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
        .collect(),
        helpers: Default::default(),
        deciders: vec![],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker() {
    let Ok(mode) = std::env::var("MAGI_EVENT_TURN_WORKER") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("HOME").expect("private home"));
    let mind = match mode.as_str() {
        "scoped" => {
            let call = magi_testkit::mind::call_lines("private-call", "private-write", "{}");
            Mind::turns(
                "event-scoped",
                &[
                    &[&call[0], &call[1], &call[2]],
                    &[
                        &magi_testkit::mind::text_line("private-answer"),
                        &magi_testkit::mind::stop_line(),
                    ],
                ],
            )
        }
        "cancel" => Mind::stalling(
            "event-cancel",
            &[&magi_testkit::mind::text_line("private-prefix")],
        ),
        "queue" => Mind::controlled("event-queue"),
        "failure" => Mind::saying(
            "event-failure",
            &[&magi_testkit::mind::failed_line("private-error", "invalid")],
        ),
        _ => Mind::answering("event-success", "private-answer"),
    };
    let socket = root.join("s");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let mut session = Session::recorded(SessionId::new("private-session"), vec![]);
    if mode == "replay" {
        session
            .commit(Entry::User {
                id: magi_proto::MessageId::new("private-old"),
                text: "private-history".into(),
                aside: String::new(),
                images: vec![],
            })
            .expect("historical entry");
        session = Session::recorded(
            SessionId::new("private-session"),
            session.entries().to_vec(),
        );
    }
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
    let memory = if mode == "store_failure" {
        Some(
            magi_testkit::memory::Failing::start(&root.join("memory.sock"))
                .await
                .expect("failing memory"),
        )
    } else {
        None
    };
    let mut configured = backend(&mind, &root);
    if mode == "scoped" {
        configured.grants.push(magi_proto::permit::Grant {
            verb: "write".into(),
            scope: magi_proto::permit::Scope::Directory {
                path: root.display().to_string(),
            },
        });
        configured.tools.push((
            "private-scope.lua".into(),
            format!(
                r#"
            magi.tool("private-write", {{
                transport = {{ kind = "lua" }},
                parameters = {{ type = "object", properties = {{}} }},
                run = function()
                    local ok, why = magi.fs.write({:?}, "private-written")
                    return {{ content = ok and "private-ok" or why, is_error = not ok }}
                end
            }})
        "#,
                root.join("private-result").display().to_string()
            ),
        ));
    }
    let host = tokio::spawn(magi_host::serve_on(
        listener,
        session,
        if mode == "missing" {
            None
        } else {
            Some(configured)
        },
        magi_host::catalog::Catalog::empty(),
        memory.as_ref().map(|memory| memory.socket().to_owned()),
    ));
    let stream = magi_ipc::connect(&socket).await.expect("connect");
    let (read, write) = stream.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor::ZERO,
            draws: false,
            key: magi_ipc::read_key(&socket),
        })
        .await
        .expect("attach");
    let snapshot = until(&mut reader, |event| {
        matches!(event, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    if let HarnessEvent::SessionSnapshot { entries, .. } = snapshot {
        assert!(entries.is_empty());
    }
    if !matches!(mode.as_str(), "disabled" | "full" | "closed") {
        let connected = drain(receiver.as_ref().expect("receiver"));
        assert_eq!(connected.len(), 1);
        let client = &connected[0];
        assert!(client.correlation.request_id.is_some());
        assert_eq!(
            client.correlation,
            Correlation {
                request_id: client.correlation.request_id,
                ..Correlation::default()
            }
        );
        assert_eq!(
            client.fact,
            Fact::Client {
                outcome: ClientOutcome::Connected
            }
        );
    }
    if mode == "replay" {
        let replayed = until(&mut reader, |event| {
            matches!(event, HarnessEvent::UserMessage { .. })
        })
        .await;
        assert!(
            matches!(replayed, HarnessEvent::UserMessage { id, text, .. }
            if id.as_str() == "private-old" && text == "private-history")
        );
        assert!(drain(receiver.as_ref().expect("receiver")).is_empty());
    }
    let prompt = |text: &str| UiCommand::SubmitPrompt {
        text: text.into(),
        aside: "private-aside".into(),
        images: vec![],
    };
    let refused_id = if mode.starts_with("preparation_") {
        writer
            .write(&UiCommand::SubmitTracked {
                request: 77,
                text: if mode == "preparation_malformed" {
                    "/skill ../private"
                } else {
                    "/skill review"
                }
                .into(),
                aside: "private-aside".into(),
                images: if mode == "preparation_image" {
                    vec![magi_model::Image {
                        data: "private-invalid".into(),
                        media_type: "image/png".into(),
                    }]
                } else {
                    vec![]
                },
            })
            .await
            .expect("preparation refusal");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event: HarnessEvent = reader.read().await.expect("native reply");
                assert!(!matches!(
                    event,
                    HarnessEvent::TurnStarted { .. }
                        | HarnessEvent::UserMessage { .. }
                        | HarnessEvent::AssistantDelta { .. }
                ));
                if matches!(event, HarnessEvent::PromptRefused { request: 77, .. }) {
                    break;
                }
            }
        })
        .await
        .expect("refusal deadline");
        assert_eq!(mind.asked(), 0);
        let events = drain(receiver.as_ref().expect("receiver"));
        let expected = if mode == "preparation_history" {
            vec![TurnOutcome::Preparing, TurnOutcome::Refused]
        } else {
            vec![TurnOutcome::Refused]
        };
        assert_eq!(events.len(), expected.len());
        let id = events[0].correlation.turn_id.expect("preparation ID");
        for (event, outcome) in events.into_iter().zip(expected) {
            assert_eq!(
                event.correlation,
                Correlation {
                    turn_id: Some(id),
                    ..Correlation::default()
                }
            );
            let Fact::LogicalTurn {
                outcome: actual,
                duration_ms,
            } = event.fact
            else {
                panic!("logical turn")
            };
            assert_eq!(actual, outcome);
            assert_eq!(duration_ms.is_some(), outcome == TurnOutcome::Refused);
        }
        Some(id)
    } else {
        None
    };
    writer
        .write(&prompt("private-prompt"))
        .await
        .expect("submit");
    let mut cancelled_pid = None;
    if mode == "cancel" || mode == "queue" {
        until(&mut reader, |event| {
            matches!(event, HarnessEvent::AssistantDelta { .. })
        })
        .await;
        if mode == "cancel" {
            tokio::time::timeout(Duration::from_secs(3), async {
                while mind.stalled_pid().is_none() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("provider PID");
            cancelled_pid = mind.stalled_pid();
            writer
                .write(&UiCommand::Interrupt)
                .await
                .expect("interrupt");
        } else {
            writer
                .write(&prompt("private-queued"))
                .await
                .expect("queue");
            writer
                .write(&UiCommand::SubmitTracked {
                    request: 42,
                    text: "private-busy".into(),
                    aside: String::new(),
                    images: vec![],
                })
                .await
                .expect("tracked busy");
            until(&mut reader, |event| {
                matches!(event, HarnessEvent::PromptRefused { request: 42, .. })
            })
            .await;
            mind.release(0);
        }
    }
    if mode == "store_failure" {
        until(&mut reader, |event| {
            matches!(
                event,
                HarnessEvent::AssistantEnded {
                    stop_reason: magi_proto::StopReason::EndTurn,
                    error: None,
                    ..
                }
            )
        })
        .await;
        until(&mut reader, |event| {
            matches!(
                event,
                HarnessEvent::AssistantEnded {
                    stop_reason: magi_proto::StopReason::Error,
                    error: Some(_),
                    ..
                }
            )
        })
        .await;
    }
    until(&mut reader, |event| {
        matches!(event, HarnessEvent::TurnFinished { .. })
    })
    .await;
    if mode == "queue" {
        until(&mut reader, |event| {
            matches!(event, HarnessEvent::AssistantDelta { .. })
        })
        .await;
        mind.release(1);
        until(&mut reader, |event| {
            matches!(event, HarnessEvent::TurnFinished { .. })
        })
        .await;
        assert!(!mind.overlapped());
        assert_eq!(mind.asks().len(), 2);
    }
    if let Some(pid) = cancelled_pid {
        tokio::time::timeout(Duration::from_secs(3), async {
            while std::path::Path::new(&format!("/proc/{pid}")).exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("provider reaped");
    }
    if mode == "store_failure" {
        assert_eq!(mind.asks().len(), 1);
        assert!(memory.as_ref().expect("failing memory").failed_writes() > 0);
    }
    if mode == "scoped" {
        assert_eq!(mind.asked(), 2);
        assert_eq!(
            std::fs::read_to_string(root.join("private-result")).expect("useful write"),
            "private-written"
        );
    }
    if mode == "disabled" {
        let receiver = enable_local().expect("activate after work");
        assert!(drain(&receiver).is_empty());
        assert_eq!(loss(), Loss::default());
    } else if mode == "closed" {
        assert_eq!(loss().closed, 4);
    } else if mode == "full" {
        assert_eq!(loss().full, 4);
        assert_eq!(
            drain(receiver.as_ref().expect("receiver")).len(),
            QUEUED_EVENTS
        );
        assert_eq!(mind.asks().len(), 1);
    } else {
        let events = drain(receiver.as_ref().expect("receiver"));
        let mut turns: BTreeMap<String, Vec<TurnOutcome>> = BTreeMap::new();
        let mut authorizations = Vec::new();
        for event in events {
            assert_ne!(event.correlation.turn_id, refused_id);
            let id = String::from(event.correlation.turn_id.expect("local turn"));
            let mut correlation = event.correlation;
            correlation.turn_id = None;
            if let Fact::Authorization { outcome } = event.fact {
                assert_eq!(mode, "scoped");
                assert_eq!(outcome, AuthorizationOutcome::StandingGrant);
                assert!(correlation.request_id.take().is_some());
                assert_eq!(correlation, Correlation::default());
                authorizations.push(id);
                continue;
            }
            assert_eq!(correlation, Correlation::default());
            let Fact::LogicalTurn {
                outcome,
                duration_ms,
            } = event.fact
            else {
                panic!("unexpected fact")
            };
            assert_eq!(
                duration_ms.is_some(),
                !matches!(
                    outcome,
                    TurnOutcome::Admitted | TurnOutcome::Queued | TurnOutcome::Started
                )
            );
            turns.entry(id).or_default().push(outcome);
        }
        if mode == "scoped" {
            assert_eq!(authorizations.len(), 1);
            assert_eq!(authorizations.first(), turns.keys().next());
        } else {
            assert!(authorizations.is_empty());
        }
        if mode == "queue" {
            assert_eq!(turns.len(), 3);
            for expected in [
                vec![
                    TurnOutcome::Admitted,
                    TurnOutcome::Started,
                    TurnOutcome::Completed,
                ],
                vec![
                    TurnOutcome::Admitted,
                    TurnOutcome::Queued,
                    TurnOutcome::Started,
                    TurnOutcome::Completed,
                ],
                vec![TurnOutcome::Refused],
            ] {
                assert!(
                    turns.values().any(|values| values == &expected),
                    "{turns:?}"
                );
            }
        } else {
            assert_eq!(turns.len(), 1);
            let terminal = match mode.as_str() {
                "failure" | "missing" | "store_failure" => TurnOutcome::Failed,
                "cancel" => TurnOutcome::Cancelled,
                _ => TurnOutcome::Completed,
            };
            assert_eq!(
                turns.values().next().expect("turn"),
                &[TurnOutcome::Admitted, TurnOutcome::Started, terminal]
            );
        }
    }
    drop(reader);
    drop(writer);
    host.abort();
    let _ = host.await;
}

#[test]
fn native_turn_events_preserve_outcomes_queueing_and_replay() {
    for mode in [
        "success",
        "scoped",
        "failure",
        "missing",
        "store_failure",
        "cancel",
        "queue",
        "replay",
        "disabled",
        "full",
        "closed",
        "preparation_malformed",
        "preparation_history",
        "preparation_image",
    ] {
        let root = Scratch::new("event-turns", mode);
        for dir in ["home", "config", "cache", "data", "run"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(dir))
                .expect("private owned directory");
        }
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "worker", "--nocapture"])
            .env_clear()
            .env("MAGI_EVENT_TURN_WORKER", mode)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("PATH", std::env::var_os("PATH").expect("test PATH"))
            .env("TMPDIR", &*root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("isolated worker");
        let deadline = Instant::now() + Duration::from_secs(60);
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
