#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::{
    Arc, BYTES, COUNT, Catalog, Entry, Mutex, Queued as Prepared, RwLock, TurnOutcome, UiCommand,
    request,
    tests::{fixture, prompt, queued, released},
};
use crate::events::{self, Correlation, Event, Fact, Loss, Receiver, TurnOutcome::*};
use magi_proto::HarnessEvent;
use std::{
    collections::BTreeMap,
    process::Command,
    time::{Duration, Instant},
};

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut result = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        assert!(bytes.len() <= events::MAX_EVENT_BYTES);
        let text = std::str::from_utf8(&bytes).expect("JSON");
        for secret in ["private", "HOME", "PATH", "/skill"] {
            assert!(!text.contains(secret), "content-free event");
        }
        result.push(events::decode(&bytes).expect("strict event"));
    }
    result
}

fn grouped(events: Vec<Event>) -> BTreeMap<String, Vec<TurnOutcome>> {
    let mut result = BTreeMap::<_, Vec<_>>::new();
    for event in events {
        let id = event.correlation.turn_id.expect("opaque turn ID");
        assert_eq!(
            event.correlation,
            Correlation {
                turn_id: Some(id),
                ..Correlation::default()
            }
        );
        let Fact::LogicalTurn {
            outcome,
            duration_ms,
        } = event.fact
        else {
            panic!("logical turn")
        };
        assert_eq!(
            duration_ms.is_some(),
            !matches!(outcome, Preparing | Admitted | Queued | Started)
        );
        result.entry(String::from(id)).or_default().push(outcome);
    }
    result
}

fn expect(receiver: &Receiver, outcomes: &[TurnOutcome]) {
    let groups = grouped(drain(receiver));
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups.values().next().expect("one turn"), outcomes);
}

async fn finished(events: &mut tokio::sync::broadcast::Receiver<HarnessEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !matches!(
            events.recv().await.expect("native event"),
            HarnessEvent::TurnFinished { .. }
        ) {}
    })
    .await
    .expect("finished turn");
}

fn probe() -> Option<String> {
    if let Ok(mode) = std::env::var("MAGI_EVENT_PREPARATION_PROBE") {
        return Some(mode);
    }
    for mode in [
        "immediate",
        "malformed",
        "oversized",
        "invalid_image",
        "cancel",
        "drop_ticket",
        "invalidate",
        "deadline",
        "missing_history",
        "handoff",
        "session_queue",
        "capacity",
        "bytes",
        "closed_queue",
        "abort",
        "maintenance",
        "disabled",
        "late_activation",
        "full",
        "closed",
    ] {
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args([
                "--exact",
                "preparing::observing::native_preparation_preserves_trace_and_admission_ownership",
            ])
            .env("MAGI_EVENT_PREPARATION_PROBE", mode)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("probe");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if child.try_wait().expect("status").is_some() {
                let output = child.wait_with_output().expect("probe output");
                assert!(
                    output.status.success(),
                    "mode {mode}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout)
                        .contains("test result: ok. 1 passed; 0 failed; 0 ignored;")
                );
                break;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("mode {mode} timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    None
}

#[tokio::test(start_paused = true)]
async fn native_preparation_preserves_trace_and_admission_ownership() {
    let Some(mode) = probe() else {
        return;
    };
    let mut receiver = if matches!(mode.as_str(), "disabled" | "late_activation") {
        None
    } else {
        Some(events::enable_local().expect("activation"))
    };
    if mode == "full" {
        for _ in 0..events::QUEUED_EVENTS {
            assert_eq!(
                events::emit(|| (
                    Correlation::default(),
                    Fact::Client {
                        outcome: events::ClientOutcome::Connected
                    }
                )),
                events::Emitted::Queued
            );
        }
    }
    if mode == "closed" {
        drop(receiver.take());
    }
    let (mut preparing, session) = fixture();
    let mut native = session.lock().await.subscribe();
    match mode.as_str() {
        "immediate" => {
            let command = UiCommand::SubmitTracked {
                request: 42,
                text: "private-prompt".into(),
                aside: "private-aside".into(),
                images: vec![],
            };
            let Prepared::Immediate(command, tracking, mut trace) =
                preparing.queue(command).expect("immediate")
            else {
                panic!("immediate")
            };
            assert_eq!(tracking, Some(42));
            assert!(drain(receiver.as_ref().expect("receiver")).is_empty());
            crate::turning::submit(
                &session,
                request(command).expect("request").tracked(tracking),
                &RwLock::new(None),
                &Catalog::empty(),
                &Arc::new(Mutex::new(None)),
                &[],
                &mut trace,
            )
            .await
            .expect("admitted");
            finished(&mut native).await;
            drop(trace);
            expect(
                receiver.as_ref().expect("receiver"),
                &[Admitted, Started, Failed],
            );
        }
        "malformed" | "oversized" | "invalid_image" => {
            let command = match mode.as_str() {
                "malformed" => prompt("/skill ../private-escape"),
                "oversized" => prompt(&format!("/skill review {}", "x".repeat(4 * 1024 * 1024))),
                _ => UiCommand::SubmitPrompt {
                    text: "/skill review".into(),
                    aside: String::new(),
                    images: vec![magi_model::Image {
                        data: "private-invalid".into(),
                        media_type: "image/png".into(),
                    }],
                },
            };
            assert!(preparing.queue(command).is_err());
            expect(receiver.as_ref().expect("receiver"), &[Refused]);
            assert!(session.lock().await.entries().is_empty());
        }
        "drop_ticket" => {
            let held = preparing.epoch.boundary.lock().await;
            let ticket = queued(&preparing, "/skill review");
            drop(ticket);
            released(&preparing).await;
            drop(held);
            expect(
                receiver.as_ref().expect("receiver"),
                &[Preparing, Cancelled],
            );
            assert!(session.lock().await.entries().is_empty());
        }
        "cancel" | "invalidate" | "deadline" => {
            let held = preparing.epoch.boundary.lock().await;
            let mut ticket = queued(&preparing, "/skill review");
            match mode.as_str() {
                "cancel" => {
                    ticket.cancel.request();
                }
                "invalidate" => preparing.invalidate(),
                _ => tokio::time::advance(Duration::from_secs(46)).await,
            }
            assert!(ticket.receive().await.is_err());
            drop(held);
            drop(ticket);
            released(&preparing).await;
            expect(
                receiver.as_ref().expect("receiver"),
                &[
                    Preparing,
                    if mode == "deadline" {
                        Expired
                    } else {
                        Cancelled
                    },
                ],
            );
            assert!(session.lock().await.entries().is_empty());
        }
        "missing_history" => {
            let mut ticket = queued(&preparing, "/skill review");
            assert!(
                ticket
                    .receive()
                    .await
                    .expect_err("missing history")
                    .contains("history store")
            );
            drop(ticket);
            released(&preparing).await;
            expect(receiver.as_ref().expect("receiver"), &[Preparing, Refused]);
            assert!(session.lock().await.entries().is_empty());
        }
        "handoff" | "disabled" | "late_activation" | "full" | "closed" | "session_queue" => {
            if mode == "session_queue" {
                session
                    .lock()
                    .await
                    .admit(crate::session::admission::Request::Declare)
                    .expect("busy session");
            }
            let held = preparing.epoch.boundary.lock().await;
            let mut workflow = queued(&preparing, "/skill review");
            let mut ordinary = queued(&preparing, "private-prompt");
            if mode == "late_activation" {
                receiver = Some(events::enable_local().expect("late activation"));
            }
            drop(held);
            assert!(workflow.receive().await.is_err());
            assert!(ordinary.receive().await.is_ok());
            drop((workflow, ordinary));
            released(&preparing).await;
            if mode == "session_queue" {
                assert!(session.lock().await.entries().is_empty());
                let mut collected = drain(receiver.as_ref().expect("receiver"));
                drop(preparing);
                tokio::task::yield_now().await;
                drop(session);
                collected.extend(drain(receiver.as_ref().expect("receiver")));
                let groups = grouped(collected);
                assert_eq!(groups.len(), 2);
                assert!(groups.values().any(|v| v == &[Preparing, Refused]));
                assert!(
                    groups
                        .values()
                        .any(|v| v == &[Preparing, Admitted, Queued, Cancelled])
                );
                return;
            }
            finished(&mut native).await;
            let held = session.lock().await;
            assert!(held.entries().iter().any(
                |entry| matches!(entry, Entry::User { text, .. } if text == "private-prompt")
            ));
            assert!(
                !held
                    .entries()
                    .iter()
                    .any(|entry| matches!(entry, Entry::Workflow { .. }))
            );
            drop(held);
            if mode == "disabled" {
                receiver = Some(events::enable_local().expect("after work"));
            }
            if mode == "closed" {
                assert_eq!(
                    events::loss(),
                    Loss {
                        closed: 6,
                        ..Loss::default()
                    }
                );
                return;
            }
            let observed = drain(receiver.as_ref().expect("receiver"));
            if mode == "full" {
                assert_eq!(observed.len(), events::QUEUED_EVENTS);
                assert!(
                    observed
                        .iter()
                        .all(|e| matches!(e.fact, Fact::Client { .. }))
                );
                assert_eq!(
                    events::loss(),
                    Loss {
                        full: 6,
                        ..Loss::default()
                    }
                );
                return;
            }
            if matches!(mode.as_str(), "disabled" | "late_activation") {
                assert!(observed.is_empty());
            } else {
                let groups = grouped(observed);
                assert_eq!(groups.len(), 2);
                assert!(groups.values().any(|v| v == &[Preparing, Refused]));
                assert!(
                    groups
                        .values()
                        .any(|v| v == &[Preparing, Admitted, Started, Failed])
                );
            }
        }
        "capacity" => {
            let held = preparing.epoch.boundary.lock().await;
            let mut tickets = vec![queued(&preparing, "/skill review")];
            let mut groups = grouped(drain(receiver.as_ref().expect("receiver")));
            for _ in 1..COUNT {
                tickets.push(queued(&preparing, "private-prompt"));
                groups.extend(grouped(drain(receiver.as_ref().expect("receiver"))));
            }
            assert_eq!(groups.len(), COUNT);
            assert!(preparing.queue(prompt("private-over-count")).is_err());
            expect(receiver.as_ref().expect("receiver"), &[Refused]);
            preparing.invalidate();
            drop(held);
            for ticket in &mut tickets {
                assert!(ticket.receive().await.is_err());
            }
            let terminal = grouped(drain(receiver.as_ref().expect("receiver")));
            assert_eq!(terminal.len(), COUNT);
            for (id, outcome) in terminal {
                assert_eq!(groups.get(&id).expect("same owner"), &[Preparing]);
                assert_eq!(outcome, [Cancelled]);
            }
            drop(tickets);
            released(&preparing).await;
        }
        "bytes" => {
            let held = preparing
                .bytes
                .clone()
                .try_acquire_many_owned(BYTES as u32)
                .expect("reserve bytes");
            assert!(preparing.queue(prompt("/skill review")).is_err());
            expect(receiver.as_ref().expect("receiver"), &[Refused]);
            assert_eq!(preparing.count.available_permits(), COUNT);
            drop(held);
        }
        "closed_queue" => {
            preparing.running.abort();
            let _ = (&mut preparing.running).await;
            assert!(preparing.queue(prompt("/skill review")).is_err());
            expect(receiver.as_ref().expect("receiver"), &[Preparing, Refused]);
        }
        "abort" => {
            let state = preparing.epoch.clone();
            let held = state.boundary.lock().await;
            let first = queued(&preparing, "/skill review");
            let second = queued(&preparing, "private-prompt");
            drop(preparing);
            tokio::task::yield_now().await;
            let groups = grouped(drain(receiver.as_ref().expect("receiver")));
            assert_eq!(groups.len(), 2);
            assert!(groups.values().all(|v| v == &[Preparing, Unknown]));
            drop((held, first, second));
            assert!(session.lock().await.entries().is_empty());
            return;
        }
        "maintenance" => {
            let held = preparing.epoch.boundary.lock().await;
            let reservation = preparing
                .count
                .clone()
                .try_acquire_owned()
                .expect("force preparation");
            let mut tickets = vec![];
            for command in [
                UiCommand::DeclareNeeds,
                UiCommand::TakeGrants { grants: vec![] },
                UiCommand::Arrived {
                    who: "private-agent".into(),
                    kin: "private-kin".into(),
                    sort: "private-sort".into(),
                    text: "private-report".into(),
                },
                UiCommand::RefreshWorkflows,
            ] {
                let Prepared::Waiting(ticket) = preparing.queue(command).expect("queue") else {
                    panic!("waiting")
                };
                tickets.push(ticket);
            }
            preparing.invalidate();
            drop(held);
            for ticket in &mut tickets {
                assert!(ticket.receive().await.is_err());
            }
            drop((tickets, reservation));
            released(&preparing).await;
            assert!(drain(receiver.as_ref().expect("receiver")).is_empty());
        }
        _ => panic!("unknown mode"),
    }
    assert_eq!(events::loss(), Loss::default());
}
