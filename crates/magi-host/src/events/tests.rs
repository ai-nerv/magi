#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::*;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

const GOLDEN: &str = include_str!("golden.json");

#[test]
fn policy_decisions_encode_fixed_sources_without_secret_labels() {
    use PolicyDecision as P;
    let cases = [
        (P::RuleDenied {}, json!({"kind":"rule_denied"})),
        (P::LockedDenied {}, json!({"kind":"locked_denied"})),
        (
            P::LocalAllowed {
                mode: LocalPolicy::Auto,
            },
            json!({"kind":"local_allowed","mode":"auto"}),
        ),
        (
            P::LocalAllowed {
                mode: LocalPolicy::Edits,
            },
            json!({"kind":"local_allowed","mode":"edits"}),
        ),
        (
            P::Model {
                outcome: PolicyOutcome::Allowed,
            },
            json!({"kind":"model","outcome":"allowed"}),
        ),
        (
            P::Model {
                outcome: PolicyOutcome::Denied,
            },
            json!({"kind":"model","outcome":"denied"}),
        ),
    ];
    for (decision, value) in cases {
        assert_eq!(serde_json::to_value(decision).expect("encoding"), value);
        assert_eq!(
            serde_json::from_value::<P>(value.clone()).expect("decoding"),
            decision
        );
        for field in ["path", "rule", "model", "reason", "confidence"] {
            let mut hostile = value.clone();
            hostile[field] = json!("private-value");
            assert!(serde_json::from_value::<P>(hostile).is_err());
        }
    }
    for (reason, name) in [
        (DelegationReason::Rule, "rule"),
        (DelegationReason::AmbiguousRule, "ambiguous_rule"),
        (DelegationReason::AskMode, "ask_mode"),
        (DelegationReason::EditsMode, "edits_mode"),
        (DelegationReason::JudgeUnavailable, "judge_unavailable"),
        (DelegationReason::JudgeUncertain, "judge_uncertain"),
        (DelegationReason::RefusalLimit, "refusal_limit"),
    ] {
        for (outcome, label) in [
            (PolicyOutcome::Allowed, "allowed"),
            (PolicyOutcome::Denied, "denied"),
        ] {
            let fact = Fact::Policy {
                decision: P::Delegated { reason, outcome },
            };
            let golden = json!({"kind":"policy","decision":{"kind":"delegated","reason":name,"outcome":label}});
            assert_eq!(serde_json::to_value(fact).expect("encoding"), golden);
            assert_eq!(
                serde_json::from_value::<Fact>(golden).expect("decoding"),
                fact
            );
        }
    }
    for value in [
        json!({"kind":"rule_denied","outcome":"allowed"}),
        json!({"kind":"local_allowed","mode":"locked"}),
        json!({"kind":"delegated","reason":"human","outcome":"allowed"}),
        json!({"kind":"model","outcome":"unknown"}),
    ] {
        assert!(serde_json::from_value::<P>(value).is_err());
    }
}

#[test]
fn local_identities_are_distinct_and_exhaust_without_wrapping() {
    let (producer, _) = pair();
    let mut ids = HashSet::new();
    for _ in 0..1024 {
        assert!(ids.insert(String::from(producer.identity().expect("identity"))));
    }
    producer.identities.store(u64::MAX - 1, Ordering::Relaxed);
    assert!(producer.identity().is_some());
    assert!(producer.identity().is_none());
    assert!(producer.identity().is_none());
    assert_eq!(producer.identities.load(Ordering::Relaxed), u64::MAX);
    assert_eq!(producer.loss.snapshot().exhausted, 2);
}

#[test]
fn receiver_shutdown_counts_queued_but_not_dequeued_events() {
    let (producer, receiver) = pair();
    for _ in 0..3 {
        assert_eq!(producer.emit(prepared), Emitted::Queued);
    }
    receiver.try_recv().expect("one event handed to consumer");
    assert_eq!(producer.loss.snapshot().abandoned, 0);
    drop(receiver);
    assert_eq!(producer.loss.snapshot().abandoned, 2);
    assert_eq!(producer.emit(prepared), Emitted::Closed);
    assert_eq!(producer.loss.snapshot().closed, 1);
    assert_eq!(producer.loss.snapshot().abandoned, 2);
}

#[test]
fn receiver_shutdown_racing_production_accounts_for_every_attempt() {
    let (producer, receiver) = pair();
    let producer = Arc::new(producer);
    let barrier = Arc::new(Barrier::new(2));
    let task = std::thread::spawn({
        let (producer, barrier) = (producer.clone(), barrier.clone());
        move || {
            barrier.wait();
            for _ in 0..1000 {
                assert!(matches!(
                    producer.emit(prepared),
                    Emitted::Queued | Emitted::Full | Emitted::Closed
                ));
            }
        }
    });
    barrier.wait();
    drop(receiver);
    task.join().expect("producer settled after shutdown");
    let loss = producer.loss.snapshot();
    assert_eq!(loss.abandoned + loss.full + loss.closed, 1000);
    assert_eq!(loss.encoding + loss.exhausted, 0);
}

fn identity() -> Id {
    "00112233445566778899aabbccddeeff"
        .to_owned()
        .try_into()
        .expect("opaque ID")
}
fn prepared() -> (Correlation, Fact) {
    (
        Correlation::default(),
        Fact::LogicalTurn {
            outcome: TurnOutcome::Cancelled,
            duration_ms: None,
        },
    )
}
fn pair() -> (Emitter, Receiver) {
    Emitter::new(identity(), [0, 7, 5])
}

#[test]
fn golden_is_exact_and_missing_measurements_remain_null() {
    let (producer, receiver) = pair();
    assert_eq!(producer.emit(prepared), Emitted::Queued);
    let bytes = receiver.try_recv().expect("queued event");
    assert_eq!(bytes, GOLDEN.trim_end().as_bytes());
    let event = decode(&bytes).expect("independent golden schema");
    assert_eq!(event.producer, identity());
    assert_eq!(event.sequence.get(), 1);
    assert_eq!(event.correlation, Correlation::default());
    assert_eq!(event.fact, prepared().1);
}

#[test]
fn revision_owner_kinds_fields_and_numeric_states_are_strict() {
    let original: Value = serde_json::from_str(GOLDEN).expect("golden");
    for (pointer, replacement) in [
        ("/schema", json!(2)),
        ("/schema", json!(-1)),
        ("/schema", json!("1")),
        ("/program", json!("melchior")),
        ("/sequence", json!(0)),
        ("/sequence", json!(-1)),
        ("/sequence", json!(1.5)),
        ("/program_version", json!([0, 7, 65536])),
        ("/fact/kind", json!("provider_attempt_ended")),
        ("/fact/outcome", json!("secret-error-canary")),
        ("/fact/duration_ms", json!(-1)),
        ("/fact/duration_ms", json!(1.5)),
        ("/correlation/run_id", json!("/private/secret-path")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).expect("known pointer") = replacement;
        let error = decode(&serde_json::to_vec(&changed).expect("JSON")).expect_err(pointer);
        assert_eq!(error, DecodeError::Invalid);
        assert!(!format!("{error:?}").contains("secret"));
    }
    for pointer in ["", "/fact", "/correlation"] {
        let mut changed = original.clone();
        changed
            .pointer_mut(pointer)
            .expect("object")
            .as_object_mut()
            .expect("map")
            .insert("private-content".into(), json!("secret-header-canary"));
        assert_eq!(
            decode(&serde_json::to_vec(&changed).expect("JSON")),
            Err(DecodeError::Invalid)
        );
    }
    let duplicate = GOLDEN.replacen("\"schema\":1", "\"schema\":1,\"schema\":1", 1);
    assert_eq!(decode(duplicate.as_bytes()), Err(DecodeError::Invalid));
}

#[test]
fn opaque_identifiers_reject_hostile_and_noncanonical_labels() {
    for value in [
        "",
        "abcd",
        "00112233445566778899AABBCCDDEEFF",
        "../private/path",
        "Bearer secret-token",
        "éééééééééééééééé",
        "00112233445566778899aabbccddeeff\n",
    ] {
        assert!(Id::try_from(value.to_owned()).is_err());
    }
    let first = Id::generate().expect("OS nonce");
    let second = Id::generate().expect("new producer nonce");
    assert_ne!(first, second);
    assert_eq!(
        Id::try_from(String::from(first)).expect("canonical roundtrip"),
        first
    );
}

#[test]
fn encoded_and_decoded_limits_are_enforced_before_unbounded_growth() {
    let mut encoder = Encoded {
        bytes: [0; MAX_EVENT_BYTES],
        len: 0,
    };
    encoder
        .write_all(&[b'x'; MAX_EVENT_BYTES])
        .expect("exact limit");
    assert!(encoder.write_all(b"x").is_err());
    assert_eq!(encoder.len, MAX_EVENT_BYTES);
    let mut bytes = GOLDEN.trim_end().as_bytes().to_vec();
    bytes.resize(MAX_EVENT_BYTES, b' ');
    assert!(decode(&bytes).is_ok());
    bytes.push(b' ');
    assert_eq!(decode(&bytes), Err(DecodeError::Oversized));
}

#[test]
fn full_queue_drops_without_waiting_for_a_stalled_consumer() {
    let (producer, receiver) = pair();
    let producer = Arc::new(producer);
    for _ in 0..QUEUED_EVENTS {
        assert_eq!(producer.emit(prepared), Emitted::Queued);
    }
    let barrier = Arc::new(Barrier::new(2));
    let (done, completion) = mpsc::sync_channel(1);
    let task = std::thread::spawn({
        let (producer, barrier) = (producer.clone(), barrier.clone());
        move || {
            barrier.wait();
            assert_eq!(producer.emit(prepared), Emitted::Full);
            done.send(()).expect("useful work progressed");
        }
    });
    barrier.wait();
    completion
        .recv_timeout(Duration::from_secs(2))
        .expect("producer cannot wait for consumer");
    task.join().expect("producer");
    assert_eq!(
        producer.loss.snapshot(),
        Loss {
            full: 1,
            ..Loss::default()
        }
    );
    let mut bytes = 0;
    for index in 1..=QUEUED_EVENTS {
        let event = receiver.try_recv().expect("bounded queue");
        bytes += event.len();
        assert_eq!(decode(&event).expect("event").sequence.get(), index as u64);
    }
    assert!(bytes <= QUEUED_EVENTS * MAX_EVENT_BYTES);
    assert!(receiver.try_recv().is_err());
    assert_eq!(producer.emit(prepared), Emitted::Queued);
    assert_eq!(
        decode(&receiver.try_recv().expect("after loss"))
            .expect("event")
            .sequence
            .get(),
        258
    );
}

#[test]
fn closure_exhaustion_and_counter_saturation_are_explicit() {
    let (producer, receiver) = pair();
    drop(receiver);
    assert_eq!(producer.emit(prepared), Emitted::Closed);
    assert_eq!(producer.loss.snapshot().closed, 1);
    producer.sequence.store(u64::MAX, Ordering::Relaxed);
    assert_eq!(
        producer.emit(|| panic!("exhausted producer must not prepare payload")),
        Emitted::Exhausted
    );
    assert_eq!(producer.sequence.load(Ordering::Relaxed), u64::MAX);
    assert_eq!(producer.loss.snapshot().exhausted, 1);
    producer.loss.exhausted.store(u64::MAX, Ordering::Relaxed);
    assert_eq!(producer.emit(prepared), Emitted::Exhausted);
    assert_eq!(producer.loss.snapshot().exhausted, u64::MAX);
}

#[test]
fn concurrent_producers_have_unique_sequences_and_account_for_loss() {
    let (producer, receiver) = pair();
    let producer = Arc::new(producer);
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn({
                let producer = producer.clone();
                move || {
                    for _ in 0..1000 {
                        assert!(matches!(
                            producer.emit(prepared),
                            Emitted::Queued | Emitted::Full
                        ));
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("producer thread");
    }
    let mut sequences = HashSet::new();
    while let Ok(bytes) = receiver.try_recv() {
        assert!(sequences.insert(decode(&bytes).expect("event").sequence));
    }
    assert_eq!(sequences.len(), QUEUED_EVENTS);
    assert_eq!(producer.loss.snapshot().full, 4000 - QUEUED_EVENTS as u64);
    assert_eq!(producer.sequence.load(Ordering::Relaxed), 4001);
}

#[test]
fn process_emission_starts_disabled_and_has_only_one_local_receiver() {
    if std::env::var_os("MAGI_EVENT_PROCESS_PROBE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "events::tests::process_emission_starts_disabled_and_has_only_one_local_receiver",
            ])
            .env("MAGI_EVENT_PROCESS_PROBE", "1")
            .output()
            .expect("isolated probe");
        assert!(
            output.status.success(),
            "isolated probe failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("test result: ok. 1 passed; 0 failed; 0 ignored;"),
            "isolated probe did not execute its required case"
        );
        return;
    }
    assert_eq!(
        emit(|| panic!("disabled must not prepare payload")),
        Emitted::Disabled
    );
    assert_eq!(loss(), Loss::default());
    let receiver = enable_local().expect("explicit local activation");
    assert!(matches!(enable_local(), Err(EnableError::AlreadyEnabled)));
    assert_eq!(emit(prepared), Emitted::Queued);
    assert!(decode(&receiver.try_recv().expect("local event")).is_ok());
    drop(receiver);
    assert_eq!(emit(prepared), Emitted::Closed);
    assert_eq!(loss().closed, 1);
}

#[test]
fn typed_owner_facts_roundtrip_without_arbitrary_measurement_maps() {
    let (producer, receiver) = pair();
    for fact in [
        Fact::LogicalTurn {
            outcome: TurnOutcome::Queued,
            duration_ms: elapsed(Instant::now()),
        },
        Fact::LogicalTurn {
            outcome: TurnOutcome::Preparing,
            duration_ms: None,
        },
        Fact::LogicalTurn {
            outcome: TurnOutcome::Expired,
            duration_ms: Some(45_000),
        },
        Fact::Permission {
            outcome: PermissionOutcome::Denied,
        },
        Fact::Client {
            outcome: ClientOutcome::Disconnected,
        },
    ] {
        assert_eq!(
            producer.emit(|| (
                Correlation {
                    turn_id: Some(identity()),
                    ..Correlation::default()
                },
                fact
            )),
            Emitted::Queued
        );
        let event = decode(&receiver.try_recv().expect("event")).expect("schema");
        assert_eq!(event.fact, fact);
        assert_eq!(event.correlation.turn_id, Some(identity()));
    }
}

#[test]
fn authorization_facts_have_only_closed_typed_outcomes() {
    let (producer, receiver) = pair();
    for (outcome, label) in [
        (AuthorizationOutcome::Ungated, "ungated"),
        (AuthorizationOutcome::StandingGrant, "standing_grant"),
        (AuthorizationOutcome::ApproverAllowed, "approver_allowed"),
        (AuthorizationOutcome::ApproverDenied, "approver_denied"),
    ] {
        let fact = Fact::Authorization { outcome };
        assert_eq!(
            serde_json::to_value(fact).expect("fact"),
            json!({"kind": "authorization", "outcome": label})
        );
        assert_eq!(
            producer.emit(|| (Correlation::default(), fact)),
            Emitted::Queued
        );
        let bytes = receiver.try_recv().expect("event");
        assert_eq!(decode(&bytes).expect("schema").fact, fact);
        let mut value: Value = serde_json::from_slice(&bytes).expect("JSON");
        value["fact"]["path"] = json!("/private/path");
        assert!(decode(&serde_json::to_vec(&value).expect("JSON")).is_err());
    }
    for outcome in ["human_allowed", "standing_denied", "private", "completed"] {
        assert!(
            serde_json::from_value::<Fact>(json!({"kind":"authorization", "outcome":outcome}))
                .is_err()
        );
    }
}
