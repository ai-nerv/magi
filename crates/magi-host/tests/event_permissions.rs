#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{
    asking::{Asker, Pending},
    events::*,
};
use magi_ipc::blocking::{FrameReader, FrameWriter};
use magi_proto::{
    Cursor, HarnessEvent, ToolCallId, UiCommand,
    permit::{Action, Decision, Lifetime, Scope},
};
use magi_testkit::Scratch;
use magi_tools::approve::Approver;
use std::{
    os::unix::fs::DirBuilderExt,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn allowed() -> Decision {
    Decision::Allow {
        scope: Scope::Directory {
            path: "/private/scope".into(),
        },
        lifetime: Lifetime::Session,
    }
}

fn action() -> Action {
    Action::Read {
        path: "/private/action".into(),
    }
}

fn drain(receiver: &Receiver) -> Vec<Event> {
    let mut events = vec![];
    while let Ok(bytes) = receiver.try_recv() {
        assert!(bytes.len() <= MAX_EVENT_BYTES);
        let text = std::str::from_utf8(&bytes).expect("JSON");
        for secret in ["private", "HOME", "PATH", "subagent"] {
            assert!(!text.contains(secret));
        }
        events.push(decode(&bytes).expect("strict event"));
    }
    events
}

fn local(mode: &str) -> Decision {
    let pending = Arc::new(Pending::new());
    let stop = Arc::new(AtomicBool::new(mode == "stopped"));
    let current = mode.to_owned();
    let waiting = pending.clone();
    let stopped = stop.clone();
    let attached = mode != "unattached";
    let asker = Asker::new(
        pending.clone(),
        Box::new(move |event| {
            let HarnessEvent::PermissionAsked {
                id, action: asked, ..
            } = event
            else {
                panic!("permission")
            };
            assert_eq!(asked, action());
            let open = waiting.open();
            assert_eq!(open.len(), 1);
            assert_eq!(waiting.open(), open);
            waiting.answer(&ToolCallId::new("private-stale"), Decision::Deny);
            match current.as_str() {
                "deny" => waiting.answer(&id, Decision::Deny),
                "cancel" => stopped.store(true, Ordering::SeqCst),
                "disconnect" => waiting.cancel(),
                "panic" => panic!("private-publication-panic"),
                _ => waiting.answer(&id, allowed()),
            }
        }),
        Box::new(|| Cursor::ZERO),
        Box::new(move || attached),
    )
    .cancellable(Box::new(move || stop.load(Ordering::SeqCst)));
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        asker.ask("private-tool", &action())
    }));
    if mode == "panic" {
        assert!(answer.is_err());
        pending.cancel();
        return Decision::Deny;
    }
    assert!(pending.open().is_empty());
    pending.answer(&ToolCallId::new("private-stale"), allowed());
    answer.expect("native permission result")
}

fn forward(mode: &str) -> Decision {
    let root = std::path::PathBuf::from(std::env::var_os("HOME").expect("private HOME"));
    let socket = root.join("s");
    let stopped = Arc::new(AtomicBool::new(false));
    let mode = mode.to_owned();
    let server = if mode == "forward_missing" {
        None
    } else {
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("listener");
        let key = magi_ipc::mint_key(&socket).expect("key");
        let stop = stopped.clone();
        Some(std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("read deadline");
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .expect("write deadline");
            let UiCommand::Review {
                tool,
                action: asked,
                key: presented,
            } = FrameReader::new(&stream).read_blocking().expect("review")
            else {
                panic!("review request")
            };
            assert_eq!(tool, "subagent: private-tool");
            assert_eq!(asked, action());
            assert_eq!(presented.as_deref(), Some(key.as_str()));
            match mode.as_str() {
                "forward_allow" => FrameWriter::new(&stream)
                    .write_blocking(&allowed())
                    .expect("allow"),
                "forward_deny" => FrameWriter::new(&stream)
                    .write_blocking(&Decision::Deny)
                    .expect("deny"),
                "forward_malformed" => FrameWriter::new(&stream)
                    .write_blocking(&"private-invalid-decision")
                    .expect("malformed reply"),
                "forward_cancel" => {
                    stop.store(true, Ordering::SeqCst);
                    assert!(
                        FrameReader::new(&stream)
                            .read_blocking::<UiCommand>()
                            .is_err(),
                        "cancelled request closes"
                    );
                }
                "forward_disconnect" => {}
                _ => panic!("unknown forward mode"),
            }
        }))
    };
    let pending = Arc::new(Pending::new());
    let asker = Asker::new(
        pending.clone(),
        Box::new(|_| panic!("forwarded prompts are not local offers")),
        Box::new(|| Cursor::ZERO),
        Box::new(|| false),
    )
    .forwarded(Some(socket))
    .cancellable(Box::new(move || stopped.load(Ordering::SeqCst)));
    let decision = asker.ask("private-tool", &action());
    if let Some(server) = server {
        server.join().expect("server join");
    }
    assert_eq!(pending.counts(), (0, 0));
    decision
}

#[test]
fn worker() {
    let Ok(mode) = std::env::var("MAGI_EVENT_PERMISSION_WORKER") else {
        return;
    };
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
    let decision = if mode.starts_with("forward_") {
        forward(&mode)
    } else {
        local(&mode)
    };
    assert_eq!(
        decision,
        match mode.as_str() {
            "allow" | "forward_allow" | "disabled" | "full" | "closed" => allowed(),
            _ => Decision::Deny,
        }
    );
    if mode == "disabled" {
        let receiver = enable_local().expect("late activation");
        assert!(drain(&receiver).is_empty());
        assert_eq!(loss(), Loss::default());
        return;
    }
    if mode == "closed" {
        assert_eq!(loss().closed, 2);
        return;
    }
    let receiver = receiver.expect("receiver");
    let events = drain(&receiver);
    if mode == "full" {
        assert_eq!(events.len(), QUEUED_EVENTS);
        assert_eq!(loss().full, 2);
        return;
    }
    let expected = match mode.as_str() {
        "allow" => vec![PermissionOutcome::Offered, PermissionOutcome::Allowed],
        "deny" => vec![PermissionOutcome::Offered, PermissionOutcome::Denied],
        "cancel" => vec![PermissionOutcome::Offered, PermissionOutcome::Cancelled],
        "disconnect" => vec![PermissionOutcome::Offered, PermissionOutcome::Unavailable],
        "panic" => vec![PermissionOutcome::Unknown],
        "unattached" | "forward_missing" => vec![PermissionOutcome::Unavailable],
        "stopped" => vec![PermissionOutcome::Cancelled],
        "forward_allow" => vec![PermissionOutcome::Forwarded, PermissionOutcome::Allowed],
        "forward_deny" => vec![PermissionOutcome::Forwarded, PermissionOutcome::Denied],
        "forward_cancel" => vec![PermissionOutcome::Forwarded, PermissionOutcome::Cancelled],
        "forward_malformed" | "forward_disconnect" => {
            vec![PermissionOutcome::Forwarded, PermissionOutcome::Unavailable]
        }
        _ => panic!("unexpected mode"),
    };
    let id = events
        .first()
        .expect("event")
        .correlation
        .request_id
        .expect("request identity");
    let mut actual = vec![];
    for event in events {
        assert_eq!(
            event.correlation,
            Correlation {
                request_id: Some(id),
                ..Correlation::default()
            }
        );
        let Fact::Permission { outcome } = event.fact else {
            panic!("permission fact")
        };
        actual.push(outcome);
    }
    assert_eq!(actual, expected);
    assert_eq!(loss(), Loss::default());
}

#[test]
fn native_permissions_distinguish_answers_from_failed_delivery() {
    for mode in [
        "allow",
        "deny",
        "cancel",
        "disconnect",
        "unattached",
        "stopped",
        "panic",
        "forward_allow",
        "forward_deny",
        "forward_cancel",
        "forward_disconnect",
        "forward_malformed",
        "forward_missing",
        "disabled",
        "full",
        "closed",
    ] {
        let root = Scratch::new("event-permissions", mode);
        for dir in ["home", "config", "cache", "data", "run"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(dir))
                .expect("private directory");
        }
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "worker", "--nocapture"])
            .env_clear()
            .env("MAGI_EVENT_PERMISSION_WORKER", mode)
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
