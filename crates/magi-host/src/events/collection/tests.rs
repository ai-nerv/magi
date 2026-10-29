#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::super::{ClientOutcome, Correlation, Emitted, Emitter, Fact};
use super::*;
use serde_json::{Value, json};

fn id(n: u64) -> Id {
    Id::try_from(format!("{n:032x}")).expect("id")
}

fn collector() -> (Emitter, Collector) {
    let (emitter, receiver) = Emitter::new(id(1), [0, 7, 5]);
    (emitter, Collector::new(receiver))
}

#[test]
fn bound_producers_reject_missing_or_rewritten_lineage_before_accounting() {
    let (_, mut collector) = collector();
    let lineage = Lineage {
        turn_id: Some(id(4)),
        ask_id: Some(id(5)),
        ..Lineage::root(id(2), id(3))
    };
    let opened = collector
        .open_bound(Owner::Melchior, lineage)
        .expect("bound producer");
    let producer = opened.sender.identity();
    let mut event = usage(2, 1, 5, 1, Some(11));
    event["producer"] = json!(String::from(producer));
    event["correlation"]["run_id"] = json!(String::from(id(2)));
    event["correlation"]["agent_id"] = json!(String::from(id(3)));
    event["correlation"]["turn_id"] = json!(String::from(id(4)));
    for field in ["run_id", "agent_id", "turn_id", "ask_id"] {
        for value in [Value::Null, json!(String::from(id(99)))] {
            let mut wrong = event.clone();
            wrong["correlation"][field] = value;
            collector.accept(
                &serde_json::to_vec(&wrong).expect("event"),
                Owner::Melchior,
                producer,
                Instant::now(),
            );
        }
    }
    assert_eq!(collector.counts.invalid, 8);
    assert_eq!(collector.counts.accepted, 0);
    assert!(collector.usage.is_empty());
    collector.accept(
        &serde_json::to_vec(&event).expect("event"),
        Owner::Melchior,
        producer,
        Instant::now(),
    );
    assert_eq!(collector.counts.accepted, 1);
    assert_eq!(collector.usage.len(), 1);
    assert!(matches!(
        collector.open_bound(
            Owner::Casper,
            Lineage {
                turn_id: Some(id(10)),
                ..Lineage::default()
            }
        ),
        Err(RegisterError::Lineage)
    ));
}

fn envelope(owner: &str, producer: u64, sequence: u64, fact: Value) -> Value {
    json!({
        "schema": 1, "program": owner, "program_version": [1, 2, 3],
        "producer": format!("{producer:032x}"), "sequence": sequence,
        "correlation": {"run_id": null, "agent_id": null, "turn_id": null,
            "request_id": null, "ask_id": null, "attempt_id": null,
            "job_id": null, "tool_call_id": null}, "fact": fact
    })
}

fn usage(producer: u64, sequence: u64, ask: u64, revision: u64, input: Option<u64>) -> Value {
    let mut event = envelope(
        "melchior",
        producer,
        sequence,
        json!({
            "kind": "usage_snapshot", "revision": revision, "input_tokens": input,
            "output_tokens": 7, "cache_read_tokens": 4, "cache_write_tokens": 2,
            "reported_cost_micros": 55, "estimated_cost_micros": null, "ledger": "persisted"
        }),
    );
    event["correlation"]["ask_id"] = json!(format!("{ask:032x}"));
    event
}

fn accept(collector: &mut Collector, value: Value, owner: Owner, producer: u64, now: Instant) {
    collector.accept(
        &serde_json::to_vec(&value).expect("encode"),
        owner,
        id(producer),
        now,
    );
}

#[test]
fn independent_owner_examples_are_strict_and_channel_bound() {
    let cases = [
        (
            Owner::Magi,
            "magi",
            json!({"kind":"client", "outcome":"connected"}),
        ),
        (
            Owner::Melchior,
            "melchior",
            json!({"kind":"provider_attempt", "outcome":"cancelled", "duration_ms":null}),
        ),
        (
            Owner::Casper,
            "casper",
            json!({"kind":"mutation", "state":{"kind":"committed", "durability":"unconfirmed", "observation":"confirmed", "delivery":"unconfirmed"}}),
        ),
        (
            Owner::Balthasar,
            "balthasar",
            json!({"kind":"job_result", "outcome":"ignored_obsolete", "revision":null}),
        ),
    ];
    for (owner, name, fact) in cases {
        let value = envelope(name, 2, 1, fact);
        let bytes = serde_json::to_vec(&value).expect("JSON");
        assert!(wire::decode(&bytes, owner, id(2)).is_ok());
        assert!(matches!(
            wire::decode(&bytes, owner, id(3)),
            Err(Invalid::Producer)
        ));
        let other = if owner == Owner::Magi {
            Owner::Melchior
        } else {
            Owner::Magi
        };
        assert!(wire::decode(&bytes, other, id(2)).is_err());
        for field in ["secret", "path", "headers", "content"] {
            for level in ["envelope", "fact", "correlation"] {
                let mut malformed = value.clone();
                if level == "envelope" {
                    malformed[field] = json!("private");
                } else {
                    malformed[level][field] = json!("private");
                }
                assert!(
                    wire::decode(&serde_json::to_vec(&malformed).expect("JSON"), owner, id(2))
                        .is_err()
                );
            }
        }
        let text = serde_json::to_string(&value).expect("JSON");
        let duplicate = text.replacen("\"schema\":1", "\"schema\":1,\"schema\":1", 1);
        assert!(wire::decode(duplicate.as_bytes(), owner, id(2)).is_err());
        for (field, bad) in [
            ("schema", json!(2)),
            ("sequence", json!(0)),
            ("producer", json!("raw-label")),
            ("program_version", json!([1, 2, 65536])),
        ] {
            let mut malformed = value.clone();
            malformed[field] = bad;
            assert!(
                wire::decode(&serde_json::to_vec(&malformed).expect("JSON"), owner, id(2)).is_err()
            );
        }
    }
    assert!(matches!(
        wire::decode(&[b' '; MAX_EVENT_BYTES + 1], Owner::Magi, id(2)),
        Err(Invalid::Oversized)
    ));
}

#[test]
fn nested_mutation_and_usage_fields_cannot_hide_content() {
    for kind in ["refused", "unknown"] {
        let value = envelope(
            "casper",
            2,
            1,
            json!({"kind":"mutation", "state":{"kind":kind, "secret":"private"}}),
        );
        assert!(
            wire::decode(
                &serde_json::to_vec(&value).expect("JSON"),
                Owner::Casper,
                id(2)
            )
            .is_err()
        );
    }
    let mut value = usage(2, 1, 3, 1, Some(10));
    value["fact"]["secret"] = json!("private");
    assert!(
        wire::decode(
            &serde_json::to_vec(&value).expect("JSON"),
            Owner::Melchior,
            id(2)
        )
        .is_err()
    );
}

#[test]
fn registration_reserves_local_slot_and_rejects_rebound_identity() {
    let (_, mut collector) = collector();
    assert!(matches!(
        collector.register(Owner::Casper, id(1)),
        Err(RegisterError::Duplicate)
    ));
    let mut senders = Vec::new();
    for n in 2..=MAX_PRODUCERS as u64 {
        senders.push(collector.register(Owner::Casper, id(n)).expect("register"));
    }
    assert!(matches!(
        collector.register(Owner::Melchior, id(2)),
        Err(RegisterError::Duplicate)
    ));
    assert!(matches!(
        collector.register(Owner::Casper, id(100)),
        Err(RegisterError::Capacity)
    ));
    assert_eq!(collector.channels.len(), MAX_PRODUCERS - 1);
    assert_eq!(collector.counts().refused_producers, 3);
}

#[test]
fn bounded_round_serves_local_and_every_remote_without_second_queue() {
    let (emitter, mut collector) = collector();
    let mut busy = collector.register(Owner::Casper, id(2)).expect("busy");
    let mut quiet = collector.register(Owner::Balthasar, id(3)).expect("quiet");
    for sequence in 1..=8 {
        let event = envelope(
            "casper",
            2,
            sequence,
            json!({"kind":"tool", "outcome":"started", "duration_ms":null}),
        );
        assert_eq!(
            busy.send(&serde_json::to_vec(&event).expect("JSON")),
            Sent::Queued
        );
    }
    let event = envelope(
        "balthasar",
        3,
        1,
        json!({"kind":"memory", "outcome":"read"}),
    );
    assert_eq!(
        quiet.send(&serde_json::to_vec(&event).expect("JSON")),
        Sent::Queued
    );
    assert_eq!(
        emitter.emit(|| (
            Correlation::default(),
            Fact::Client {
                outcome: ClientOutcome::Connected
            }
        )),
        Emitted::Queued
    );
    collector.poll();
    assert_eq!(collector.counts().accepted, 3);
    collector.poll();
    assert_eq!(collector.counts().accepted, 4);
    drop(collector);
    assert_eq!(
        emitter.emit(|| (
            Correlation::default(),
            Fact::Client {
                outcome: ClientOutcome::Disconnected
            }
        )),
        Emitted::Closed
    );
    assert_eq!(
        quiet.send(&serde_json::to_vec(&event).expect("JSON")),
        Sent::Unavailable
    );
}

#[test]
fn forgery_is_rejected_before_dedup_and_does_not_poison_valid_frame() {
    let (_, mut collector) = collector();
    let mut sender = collector.register(Owner::Melchior, id(2)).expect("sender");
    for event in [
        usage(3, 1, 4, 1, Some(10)),
        usage(2, 1, 4, 1, Some(20)),
        usage(2, 1, 4, 1, Some(20)),
    ] {
        assert_eq!(
            sender.send(&serde_json::to_vec(&event).expect("JSON")),
            Sent::Queued
        );
        collector.poll();
    }
    assert_eq!(collector.counts().accepted, 1);
    assert_eq!(collector.counts().invalid, 1);
    assert_eq!(collector.counts().duplicate, 1);
    assert_eq!(
        collector.usage(id(2), id(4)).expect("usage").input_tokens,
        Some(20)
    );
}

#[test]
fn cumulative_usage_replaces_without_double_count_and_keeps_unknowns() {
    let (_, mut collector) = collector();
    let now = Instant::now();
    accept(
        &mut collector,
        usage(2, 1, 5, 1, Some(10)),
        Owner::Melchior,
        2,
        now,
    );
    accept(
        &mut collector,
        usage(2, 2, 5, 2, Some(20)),
        Owner::Melchior,
        2,
        now,
    );
    let latest = collector.usage(id(2), id(5)).expect("usage");
    assert_eq!(latest.input_tokens, Some(20));
    assert_eq!(latest.cache_read_tokens, Some(4));
    assert_eq!(latest.cache_write_tokens, Some(2));
    assert_eq!(latest.reported_cost_micros, Some(55));
    assert_eq!(latest.estimated_cost_micros, None);
    accept(
        &mut collector,
        usage(2, 3, 5, 1, Some(10)),
        Owner::Melchior,
        2,
        now,
    );
    accept(
        &mut collector,
        usage(2, 4, 5, 2, Some(999)),
        Owner::Melchior,
        2,
        now,
    );
    assert_eq!(collector.usage(id(2), id(5)), Some(latest));
    assert_eq!(collector.counts().usage_stale, 1);
    assert_eq!(collector.counts().usage_conflicts, 1);
    accept(
        &mut collector,
        usage(2, 5, 5, 3, None),
        Owner::Melchior,
        2,
        now,
    );
    assert_eq!(
        collector.usage(id(2), id(5)).expect("usage").input_tokens,
        None
    );
    accept(
        &mut collector,
        usage(3, 1, 5, 1, Some(7)),
        Owner::Melchior,
        3,
        now,
    );
    assert_eq!(collector.usage.len(), 2);
    assert_eq!(
        collector
            .usage(id(3), id(5))
            .expect("independent producer")
            .input_tokens,
        Some(7)
    );
}

#[test]
fn unknown_ask_is_not_guessed_from_other_correlations() {
    let (_, mut collector) = collector();
    let mut event = usage(2, 1, 3, 1, Some(10));
    event["correlation"]["ask_id"] = Value::Null;
    event["correlation"]["request_id"] = json!(format!("{:032x}", 3));
    accept(&mut collector, event, Owner::Melchior, 2, Instant::now());
    assert_eq!(collector.counts().usage_unattributed, 1);
    assert!(collector.usage.is_empty());
}

#[test]
fn dedup_keys_are_count_and_age_bounded_with_explicit_eviction() {
    let (_, mut collector) = collector();
    let now = Instant::now();
    let event = |sequence| {
        envelope(
            "magi",
            2,
            sequence,
            json!({"kind":"client", "outcome":"connected"}),
        )
    };
    for sequence in 1..=MAX_SEEN as u64 + 1 {
        accept(&mut collector, event(sequence), Owner::Magi, 2, now);
    }
    assert_eq!(collector.seen.len(), MAX_SEEN);
    assert_eq!(collector.order.len(), MAX_SEEN);
    assert_eq!(collector.counts().dedup_evicted, 1);
    accept(&mut collector, event(2), Owner::Magi, 2, now);
    assert_eq!(collector.counts().duplicate, 1);
    collector.poll_at(now + RETENTION);
    assert!(collector.seen.is_empty());
    assert!(collector.order.is_empty());
    assert_eq!(collector.counts().dedup_evicted, MAX_SEEN as u64 + 1);
    accept(&mut collector, event(2), Owner::Magi, 2, now + RETENTION);
    assert_eq!(collector.seen.len(), 1);
}

#[test]
fn usage_capacity_and_ttl_evictions_are_explicit() {
    let (_, mut collector) = collector();
    let now = Instant::now();
    for ask in 1..=MAX_ASKS as u64 + 1 {
        accept(
            &mut collector,
            usage(2, ask, ask, 1, Some(ask)),
            Owner::Melchior,
            2,
            now,
        );
    }
    assert_eq!(collector.usage.len(), MAX_ASKS);
    assert_eq!(collector.counts().usage_evicted, 1);
    assert!(collector.usage(id(2), id(1)).is_none());
    accept(
        &mut collector,
        usage(2, 2000, 2, 2, Some(99)),
        Owner::Melchior,
        2,
        now + Duration::from_secs(1),
    );
    collector.poll_at(now + RETENTION);
    assert_eq!(collector.usage.len(), 1);
    assert_eq!(
        collector
            .usage(id(2), id(2))
            .expect("recent update")
            .input_tokens,
        Some(99)
    );
    assert_eq!(collector.counts().usage_evicted, MAX_ASKS as u64);
    collector.poll_at(now + RETENTION + Duration::from_secs(1));
    assert!(collector.usage.is_empty());
}
