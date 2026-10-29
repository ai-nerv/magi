#![allow(clippy::expect_used)]
use magi_host::{
    events::*,
    judging::{Judge, Judged, Rules, Standing},
};
use magi_proto::{
    judging::{Advice, Mode},
    permit::{Action, Decision, Grant, Lifetime, Scope},
};
use magi_tools::approve::Approver;
use std::{
    collections::HashSet,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

struct Person {
    mode: String,
    calls: AtomicUsize,
}
impl Approver for Person {
    fn ask(&self, _: &str, _: &Action) -> Decision {
        self.calls.fetch_add(1, Ordering::Relaxed);
        assert_ne!(self.mode, "person-panic", "private-person-error");
        if self.mode == "fallback-deny" {
            Decision::Deny
        } else {
            answer()
        }
    }
}
fn answer() -> Decision {
    Decision::Allow {
        scope: Scope::Program {
            program: "private-program".into(),
        },
        lifetime: Lifetime::Session,
    }
}
struct Model {
    mode: String,
    calls: AtomicUsize,
}
impl Judge for Model {
    fn judge(&self, _: &str, _: &Action) -> Option<Advice> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        assert_ne!(self.mode, "judge-panic", "private-judge-error");
        if self.mode == "unavailable" {
            return None;
        }
        Some(Advice {
            safe: self.mode == "model-allow",
            sure: (self.mode == "uncertain").then_some(0.5),
            rule: "private-rule".into(),
            reason: "private-reason".into(),
        })
    }
}

fn drain(receiver: &Receiver) -> Vec<PolicyDecision> {
    let mut policies = Vec::new();
    let mut ids = HashSet::new();
    while let Ok(bytes) = receiver.try_recv() {
        assert!(bytes.len() <= MAX_EVENT_BYTES);
        for secret in [
            "private",
            "HOME",
            "reason\":\"private",
            "PATH",
            "private-program",
        ] {
            assert!(!String::from_utf8_lossy(&bytes).contains(secret));
        }
        let event = decode(&bytes).expect("strict event");
        let id = event.correlation.request_id.expect("local policy ID");
        assert!(ids.insert(String::from(id)));
        assert_eq!(
            event.correlation,
            Correlation {
                request_id: Some(id),
                ..Correlation::default()
            }
        );
        let Fact::Policy { decision } = event.fact else {
            panic!("policy fact")
        };
        policies.push(decision);
    }
    policies
}

#[test]
fn worker() {
    let Ok(mode) = std::env::var("MAGI_EVENT_POLICY_PROBE") else {
        return;
    };
    use DelegationReason as R;
    use PolicyDecision as P;
    use PolicyOutcome::{Allowed, Denied};
    let root = std::path::PathBuf::from(std::env::var_os("HOME").expect("fixture root"));
    let person = Arc::new(Person {
        mode: mode.clone(),
        calls: AtomicUsize::new(0),
    });
    let judge = Arc::new(Model {
        mode: mode.clone(),
        calls: AtomicUsize::new(0),
    });
    let policy_mode = match mode.as_str() {
        "locked" => Mode::Locked,
        "ask" | "person-panic" => Mode::Ask,
        "edits-edit" | "edits-ask" => Mode::Edits,
        _ => Mode::Auto,
    };
    let standing = Arc::new(Standing::starting(policy_mode));
    let mut rules = Rules::default();
    let grant = Grant {
        verb: "run".into(),
        scope: Scope::Program {
            program: "private-program".into(),
        },
    };
    if mode == "rule-deny" {
        rules.deny.push(grant.clone());
    }
    if mode == "rule-ask" || mode == "ambiguous" {
        rules.ask.push(grant);
    }
    let mut action = Action::Run {
        program: "private-program".into(),
        command: "private-program --private-value".into(),
    };
    if mode == "ambiguous" {
        action = Action::Run {
            program: "sh".into(),
            command: "sh -c \"$PRIVATE_PROGRAM\"".into(),
        };
    }
    if mode == "auto-read" {
        action = Action::Read {
            path: root.join("private-file").display().to_string(),
        };
    }
    if mode == "auto-edit" || mode == "edits-edit" {
        action = Action::Write {
            path: root.join("private-file").display().to_string(),
        };
    }
    let notice_panic = mode == "notice-panic";
    let gate = Judged::new(
        person.clone(),
        judge.clone(),
        standing.clone(),
        rules,
        &root,
        Box::new(move |_| assert!(!notice_panic, "private-notice-error")),
    );
    let receiver = if mode == "disabled" {
        None
    } else {
        Some(enable_local().expect("activation"))
    };
    let receiver = if mode == "closed" {
        drop(receiver);
        None
    } else {
        receiver
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
    if mode.ends_with("-panic") {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || gate.ask("private-tool", &action)
            ))
            .is_err()
        );
        assert!(
            drain(&receiver.expect("receiver")).is_empty(),
            "unwinding is not a completed policy decision"
        );
        return;
    }
    let mut expected = Vec::new();
    if mode.starts_with("fallback-") {
        for _ in 0..2 {
            assert_eq!(gate.ask("private-tool", &action), Decision::Deny);
            expected.push(P::Model { outcome: Denied });
        }
    }
    let decision = gate.ask("private-tool", &action);
    let (policy, delegated, judges) = match mode.as_str() {
        "rule-deny" => (P::RuleDenied {}, 0, 0),
        "locked" => (P::LockedDenied {}, 0, 0),
        "auto-read" | "auto-edit" => (
            P::LocalAllowed {
                mode: LocalPolicy::Auto,
            },
            0,
            0,
        ),
        "edits-edit" => (
            P::LocalAllowed {
                mode: LocalPolicy::Edits,
            },
            0,
            0,
        ),
        "model-allow" => (P::Model { outcome: Allowed }, 0, 1),
        "rule-ask" => (
            P::Delegated {
                reason: R::Rule,
                outcome: Allowed,
            },
            1,
            1,
        ),
        "ambiguous" => (
            P::Delegated {
                reason: R::AmbiguousRule,
                outcome: Allowed,
            },
            1,
            1,
        ),
        "ask" => (
            P::Delegated {
                reason: R::AskMode,
                outcome: Allowed,
            },
            1,
            1,
        ),
        "edits-ask" => (
            P::Delegated {
                reason: R::EditsMode,
                outcome: Allowed,
            },
            1,
            1,
        ),
        "unavailable" => (
            P::Delegated {
                reason: R::JudgeUnavailable,
                outcome: Allowed,
            },
            1,
            1,
        ),
        "uncertain" => (
            P::Delegated {
                reason: R::JudgeUncertain,
                outcome: Allowed,
            },
            1,
            1,
        ),
        "fallback-allow" => (
            P::Delegated {
                reason: R::RefusalLimit,
                outcome: Allowed,
            },
            1,
            3,
        ),
        "fallback-deny" => (
            P::Delegated {
                reason: R::RefusalLimit,
                outcome: Denied,
            },
            1,
            3,
        ),
        _ => (P::Model { outcome: Denied }, 0, 1),
    };
    expected.push(policy);
    let native = match policy {
        P::RuleDenied {}
        | P::LockedDenied {}
        | P::Model { outcome: Denied }
        | P::Delegated {
            outcome: Denied, ..
        } => Decision::Deny,
        P::Delegated { .. } => answer(),
        _ => Decision::Allow {
            scope: Scope::Once,
            lifetime: Lifetime::Session,
        },
    };
    assert_eq!(decision, native, "exact native grant preserved");
    assert_eq!(person.calls.load(Ordering::Relaxed), delegated);
    assert_eq!(judge.calls.load(Ordering::Relaxed), judges);
    if mode.starts_with("fallback-") {
        let shown = standing.as_shown(&Default::default());
        assert_eq!(shown.judged, 3);
        assert_eq!(shown.refused, if mode == "fallback-allow" { 0 } else { 3 });
    }
    if mode == "disabled" {
        assert!(drain(&enable_local().expect("late activation")).is_empty());
    } else if mode == "closed" {
        assert_eq!(
            loss(),
            Loss {
                closed: 1,
                ..Loss::default()
            }
        );
        return;
    } else if mode == "full" {
        let receiver = receiver.expect("receiver");
        for _ in 0..QUEUED_EVENTS {
            receiver.try_recv().expect("original queue");
        }
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            loss(),
            Loss {
                full: 1,
                ..Loss::default()
            }
        );
        return;
    } else {
        assert_eq!(drain(&receiver.expect("receiver")), expected);
    }
    assert_eq!(loss(), Loss::default());
}

#[test]
fn actual_policy_provenance_preserves_native_decisions() {
    for mode in [
        "rule-deny",
        "rule-ask",
        "ambiguous",
        "locked",
        "auto-read",
        "auto-edit",
        "edits-edit",
        "ask",
        "edits-ask",
        "model-allow",
        "model-deny",
        "unavailable",
        "uncertain",
        "fallback-allow",
        "fallback-deny",
        "disabled",
        "full",
        "closed",
        "judge-panic",
        "person-panic",
        "notice-panic",
    ] {
        let root = magi_testkit::Scratch::new("event-policy", mode);
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "worker", "--nocapture"])
            .env("MAGI_EVENT_POLICY_PROBE", mode)
            .env("HOME", &*root)
            .current_dir(&*root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("isolated probe");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().expect("probe status") {
                assert!(status.success(), "policy {mode}: {status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().expect("stop probe");
                child.wait().expect("reap probe");
                panic!("policy {mode} deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
