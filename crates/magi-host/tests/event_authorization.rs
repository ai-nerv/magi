#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{
    asking::{Asker, Pending},
    events::*,
    judging::{Judge, Judged, Standing},
    session::Session,
    turn::Backend,
    worker::Worker,
};
use magi_proto::{
    Cursor, Entry, HarnessEvent, SessionId,
    judging::{Advice, Mode},
    permit::{Action, Decision, Grant, Lifetime, Scope},
};
use magi_testkit::{
    Mind, Scratch,
    mind::{call_lines, stop_line, text_line},
};
use magi_tools::approve::Approver;
use std::{
    collections::HashSet,
    os::unix::fs::DirBuilderExt,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

struct Answer {
    mode: String,
    asked: AtomicUsize,
    root: String,
}

impl Approver for Answer {
    fn ask(&self, tool: &str, action: &Action) -> Decision {
        assert_eq!(tool, "fs.write");
        assert!(matches!(action, Action::Write { path } if path.starts_with(&self.root)));
        self.asked.fetch_add(1, Ordering::SeqCst);
        if denied(&self.mode) {
            Decision::Deny
        } else {
            Decision::Allow {
                scope: if self.mode == "remember" {
                    Scope::Directory {
                        path: self.root.clone(),
                    }
                } else {
                    Scope::Once
                },
                lifetime: Lifetime::Session,
            }
        }
    }
    fn overrides(&self, _: &Action) -> bool {
        self.mode == "override"
    }
    fn why(&self, _: &Action) -> Option<String> {
        Some("private-refusal".into())
    }
}

fn denied(mode: &str) -> bool {
    mode.ends_with("deny") || mode == "override"
}

struct UnusedJudge;
struct NoAdvice;
impl Judge for NoAdvice {
    fn judge(&self, _: &str, _: &Action) -> Option<Advice> {
        None
    }
}
impl Judge for UnusedJudge {
    fn judge(&self, _: &str, _: &Action) -> Option<Advice> {
        panic!("local policy does not need a model judge")
    }
}

fn approver(mode: &str, answer: &Arc<Answer>, root: &Path) -> Option<Arc<dyn Approver>> {
    if matches!(mode, "ungated" | "confined") {
        return None;
    }
    if mode.contains("prompt") {
        let pending = Arc::new(Pending::new());
        let waiting = pending.clone();
        let answer = answer.clone();
        let asker = Arc::new(Asker::new(
            pending,
            Box::new(move |event| {
                let HarnessEvent::PermissionAsked {
                    id, tool, action, ..
                } = event
                else {
                    panic!("permission request")
                };
                waiting.answer(&id, answer.ask(&tool, &action));
            }),
            Box::new(|| Cursor::ZERO),
            Box::new(|| true),
        ));
        if mode.starts_with("judged_prompt") {
            return Some(Arc::new(Judged::new(
                asker,
                Arc::new(NoAdvice),
                Arc::new(Standing::starting(Mode::Ask)),
                Default::default(),
                root,
                Box::new(|_| {}),
            )));
        }
        return Some(asker);
    }
    let policy = match mode {
        "auto" => Some(Mode::Auto),
        "edits" => Some(Mode::Edits),
        "locked_deny" => Some(Mode::Locked),
        _ => None,
    };
    if let Some(policy) = policy {
        return Some(Arc::new(Judged::new(
            answer.clone(),
            Arc::new(UnusedJudge),
            Arc::new(Standing::starting(policy)),
            Default::default(),
            root,
            Box::new(|_| {}),
        )));
    }
    Some(answer.clone())
}

fn backend(mind: &Mind, root: &Path, mode: &str) -> Backend {
    Backend {
        tools: vec![(
            "private-fixture.lua".into(),
            r#"
            magi.tool("private-write", {
                transport = { kind = "lua" },
                parameters = { type = "object", properties = {
                    first = { type = "string" }, second = { type = "string" }
                } },
                run = function(args)
                    local first, why = magi.fs.write(args.first, "private-first")
                    local second, other = magi.fs.write(args.second, "private-second")
                    return { content = first and second and "private-written" or (why or other),
                             is_error = not (first and second) }
                end
            })
        "#
            .into(),
        )],
        clients: vec![],
        tooling: magi_tools::supplier::Tooling {
            program: root.join("private-absent-tools").display().to_string(),
            ..Default::default()
        },
        cwd: root.into(),
        model: "fake/private-model".into(),
        mind: mind.program().display().to_string(),
        wants: Default::default(),
        context_window: Some(200_000),
        max_output: None,
        system: None,
        confine: mode == "confined",
        isolate: false,
        grants: if matches!(mode, "standing" | "override") {
            vec![Grant {
                verb: "write".into(),
                scope: Scope::Directory {
                    path: root.display().to_string(),
                },
            }]
        } else {
            vec![]
        },
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

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut events = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        assert!(bytes.len() <= MAX_EVENT_BYTES);
        let text = std::str::from_utf8(&bytes).expect("JSON");
        for secret in ["private", "HOME", "PATH", "fake/", "fs.write"] {
            assert!(!text.contains(secret), "content-free event");
        }
        events.push(decode(&bytes).expect("strict event"));
    }
    events
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker() {
    let Ok(mode) = std::env::var("MAGI_EVENT_AUTHORIZATION_WORKER") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("HOME").expect("private HOME"));
    let first = root.join("private-first");
    let second = root.join("private-second");
    if mode == "write_failure" {
        std::fs::create_dir(&first).expect("unwritable file target");
        std::fs::create_dir(&second).expect("unwritable file target");
    }
    let args = serde_json::json!({"first": first, "second": second}).to_string();
    let call = call_lines("private-call", "private-write", &args);
    let mind = Mind::turns(
        "event-authorization",
        &[
            &[&call[0], &call[1], &call[2]],
            &[&text_line("private-answer"), &stop_line()],
        ],
    );
    let receiver = if mode.starts_with("disabled") {
        None
    } else {
        Some(enable_local().expect("activation"))
    };
    if mode.starts_with("full") {
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
    let receiver = if mode.starts_with("closed") {
        drop(receiver);
        None
    } else {
        receiver
    };
    let answer = Arc::new(Answer {
        mode: mode.clone(),
        asked: AtomicUsize::new(0),
        root: root.display().to_string(),
    });
    let worker = Worker::gated(
        backend(&mind, &root, &mode),
        approver(&mode, &answer, &root),
        Arc::new(magi_tools::question::Unanswered),
        Arc::new(magi_tools::holding::Screenless),
        Arc::new(magi_tools::holding::Incurious),
        Arc::new(Mutex::new(None)),
    );
    let session = Arc::new(Mutex::new(Session::recorded(
        SessionId::new("private-session"),
        vec![],
    )));
    tokio::time::timeout(Duration::from_secs(10), worker.run(session.clone()))
        .await
        .expect("turn deadline")
        .expect("turn finished");
    assert_eq!(mind.asked(), 2);
    let failed = denied(&mode) || mode == "write_failure";
    if denied(&mode) {
        assert!(!first.exists() && !second.exists());
    } else if mode == "write_failure" {
        assert!(first.is_dir() && second.is_dir());
    } else {
        assert_eq!(
            std::fs::read_to_string(first).expect("first write"),
            "private-first"
        );
        assert_eq!(
            std::fs::read_to_string(second).expect("second write"),
            "private-second"
        );
    }
    let held = session.lock().await;
    let outputs: Vec<_> = held
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool {
                result: Some(result),
                ..
            } => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].is_error, failed);
    if denied(&mode) {
        assert!(outputs[0].output.contains(if mode == "locked_deny" {
            "locked"
        } else if mode.ends_with("prompt_deny") {
            "not permitted"
        } else {
            "private-refusal"
        }));
    }
    assert!(
        matches!(held.entries().last(), Some(Entry::Assistant { text, .. }) if text == "private-answer")
    );
    drop(held);
    assert_eq!(
        answer.asked.load(Ordering::SeqCst),
        match mode.as_str() {
            "ungated" | "confined" | "standing" | "auto" | "edits" | "locked_deny" => 0,
            "remember" => 1,
            _ => 2,
        }
    );
    drop(worker);
    if mode.starts_with("disabled") {
        assert!(drain(&enable_local().expect("late activation")).is_empty());
        assert_eq!(loss(), Loss::default());
        return;
    }
    if mode.starts_with("closed") {
        assert_eq!(
            loss(),
            Loss {
                closed: 2,
                ..Loss::default()
            }
        );
        return;
    }
    let events = drain(&receiver.expect("receiver"));
    if mode.starts_with("full") {
        assert_eq!(events.len(), QUEUED_EVENTS);
        assert!(
            events
                .iter()
                .all(|event| matches!(event.fact, Fact::Client { .. }))
        );
        assert_eq!(
            loss(),
            Loss {
                full: 2,
                ..Loss::default()
            }
        );
        return;
    }
    let expected = match mode.as_str() {
        "ungated" | "confined" => [AuthorizationOutcome::Ungated; 2],
        "standing" => [AuthorizationOutcome::StandingGrant; 2],
        "remember" => [
            AuthorizationOutcome::ApproverAllowed,
            AuthorizationOutcome::StandingGrant,
        ],
        _ if denied(&mode) => [AuthorizationOutcome::ApproverDenied; 2],
        _ => [AuthorizationOutcome::ApproverAllowed; 2],
    };
    let mut facts = vec![];
    for outcome in expected {
        let policy = match mode.as_str() {
            "auto" => Some(PolicyDecision::LocalAllowed {
                mode: LocalPolicy::Auto,
            }),
            "edits" => Some(PolicyDecision::LocalAllowed {
                mode: LocalPolicy::Edits,
            }),
            "locked_deny" => Some(PolicyDecision::LockedDenied {}),
            _ => None,
        };
        if let Some(decision) = policy {
            facts.push(Fact::Policy { decision });
        }
        if mode.contains("prompt") {
            facts.push(Fact::Permission {
                outcome: PermissionOutcome::Offered,
            });
            facts.push(Fact::Permission {
                outcome: if denied(&mode) {
                    PermissionOutcome::Denied
                } else {
                    PermissionOutcome::Allowed
                },
            });
        }
        if mode.starts_with("judged_prompt") {
            facts.push(Fact::Policy {
                decision: PolicyDecision::Delegated {
                    reason: DelegationReason::AskMode,
                    outcome: if denied(&mode) {
                        PolicyOutcome::Denied
                    } else {
                        PolicyOutcome::Allowed
                    },
                },
            });
        }
        facts.push(Fact::Authorization { outcome });
    }
    assert_eq!(events.len(), facts.len());
    let mut ids = HashSet::new();
    for (index, (event, fact)) in events.iter().zip(facts).enumerate() {
        assert_eq!(event.fact, fact);
        assert_eq!(event.sequence.get(), index as u64 + 1);
        let id = event
            .correlation
            .request_id
            .expect("local authorization ID");
        if matches!(
            fact,
            Fact::Permission {
                outcome: PermissionOutcome::Allowed | PermissionOutcome::Denied
            }
        ) {
            assert_eq!(event.correlation, events[index - 1].correlation);
        } else {
            assert!(ids.insert(String::from(id)));
        }
        assert_eq!(
            event.correlation,
            Correlation {
                request_id: Some(id),
                ..Correlation::default()
            }
        );
    }
    assert_eq!(loss(), Loss::default());
}

#[test]
fn native_worker_authorization_preserves_authority_and_work() {
    for mode in [
        "ungated",
        "confined",
        "allow",
        "deny",
        "standing",
        "override",
        "remember",
        "once",
        "auto",
        "edits",
        "locked_deny",
        "prompt_allow",
        "prompt_deny",
        "judged_prompt_allow",
        "judged_prompt_deny",
        "write_failure",
        "disabled_allow",
        "disabled_deny",
        "full_allow",
        "full_deny",
        "closed_allow",
        "closed_deny",
    ] {
        let root = Scratch::new("event-authorization", mode);
        for dir in ["home", "config", "cache", "data", "run"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(dir))
                .expect("private directory");
        }
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "worker", "--nocapture"])
            .env_clear()
            .env("MAGI_EVENT_AUTHORIZATION_WORKER", mode)
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
        let deadline = Instant::now() + Duration::from_secs(15);
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
