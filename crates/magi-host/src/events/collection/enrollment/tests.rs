#![allow(clippy::expect_used)]

use super::*;
use crate::events::{
    Emitter, Loss,
    collection::{Collector, Readout, Report, Sent, Summary, Worker},
};
use std::time::{Duration, Instant};

pub(super) fn id(n: u64) -> Id {
    Id::try_from(format!("{n:032x}")).expect("id")
}

pub(super) fn setup() -> (Worker, Host, Sender, Client) {
    let (_, receiver) = Emitter::new(id(1), [1, 0, 0]);
    let worker = Worker::start(Collector::new(receiver)).expect("worker");
    let lineage = Lineage::root(id(2), id(3));
    let (host, anchor, socket) = Host::pair(worker.registrar(), lineage).expect("host");
    let client = Client::adopt(socket, lineage).expect("client");
    (worker, host, anchor, client)
}

pub(super) fn wait(readout: &Readout, condition: impl Fn(Summary) -> bool) -> Summary {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let summary = readout.snapshot();
        if condition(summary) {
            return summary;
        }
        assert!(Instant::now() < deadline, "collector barrier: {summary:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn event(sender: &Sender, lineage: Lineage) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema":1, "program":"casper", "program_version":[1,0,0],
        "producer":sender.identity(), "sequence":1, "correlation":lineage,
        "fact":{"kind":"tool", "outcome":"succeeded", "duration_ms":1}
    }))
    .expect("event")
}

#[test]
fn queued_events_are_bound_at_root_before_collection_and_credits_are_not_producers() {
    let (worker, mut host, mut anchor, client) = setup();
    let readout = worker.readout();
    assert_eq!(readout.snapshot().remote_producers, 1);
    assert_eq!(host.credits.len(), CREDITS);
    let lineage = Lineage {
        turn_id: Some(id(4)),
        ask_id: Some(id(5)),
        ..host.lineage
    };
    let mut opened = client.open(Owner::Casper, lineage).expect("native launch");
    assert_eq!(opened.sender.lineage(), lineage);
    assert_eq!(
        opened.sender.send(&event(&opened.sender, lineage)),
        Sent::Queued
    );
    assert_eq!(
        opened.sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    assert_eq!(readout.snapshot().counts.accepted, 0);
    assert!(host.poll());
    wait(&readout, |s| {
        s.counts.accepted == 1 && s.retired.final_confirmed == 1
    });
    drop(opened);
    assert!(host.poll());
    assert_eq!(
        anchor.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    drop(client);
    assert!(!host.poll());
    wait(&readout, |s| s.retired.final_confirmed == 2);
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.counts.refused_producers, 0);
    assert_eq!(summary.counts.invalid, 0);
    assert!(!summary.incomplete());
}

#[test]
fn direct_and_child_admission_share_capacity_and_retirement_replenishes_it() {
    let (worker, mut host, _anchor, client) = setup();
    let registrar = worker.registrar();
    let mut direct = Vec::new();
    while registrar.available() {
        direct.push(registrar.open(Owner::Magi).expect("direct"));
    }
    assert_eq!(direct.len(), MAX_PRODUCERS - 2 - CREDITS);
    let mut first = client
        .open(Owner::Casper, host.lineage)
        .expect("first credit");
    let second = client
        .open(Owner::Melchior, host.lineage)
        .expect("second credit");
    assert!(matches!(
        client.open(Owner::Casper, host.lineage),
        Err(RegisterError::Busy)
    ));
    assert!(matches!(
        registrar.open(Owner::Magi),
        Err(RegisterError::Capacity)
    ));
    assert!(host.poll() && host.poll());
    assert!(!registrar.available());
    assert_eq!(
        worker.readout().snapshot().remote_producers,
        MAX_PRODUCERS - 1
    );
    assert_eq!(
        first.sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    let old = first.sender.identity();
    drop(first);
    assert!(host.poll());
    wait(&worker.readout(), |s| s.retired.final_confirmed == 1);
    assert!(host.poll());
    client.poll().expect("replacement offer");
    let replacement = client
        .open(Owner::Balthasar, host.lineage)
        .expect("replacement credit");
    assert_ne!(replacement.sender.identity(), old);
    assert!(host.poll());
    drop(replacement);
    drop(second);
    drop(client);
    while host.poll() {}
    drop(direct);
    let summary = worker.shutdown().expect("shutdown");
    assert!(summary.incomplete());
    assert_eq!(summary.counts.refused_producers, 1);
}

#[test]
fn stalled_root_never_blocks_launch_and_client_close_retires_held_lifetimes() {
    let (worker, mut host, _anchor, client) = setup();
    let mut first = client.open(Owner::Casper, host.lineage).expect("first");
    let second = client.open(Owner::Melchior, host.lineage).expect("second");
    let start = Instant::now();
    let mut useful = 0;
    for _ in 0..10_000 {
        assert!(matches!(
            client.open(Owner::Casper, host.lineage),
            Err(RegisterError::Busy)
        ));
        useful += 1;
    }
    assert_eq!(useful, 10_000);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(host.poll() && host.poll());
    drop(client);
    assert!(!host.poll());
    let summary = wait(&worker.readout(), |s| s.retired.unknown_loss_producers == 3);
    assert_eq!(summary.remote_producers, 0);
    assert_eq!(summary.counts.refused_producers, 0);
    assert_eq!(first.sender.send(b"after-close"), Sent::Unavailable);
    drop(first);
    drop(second);
    assert!(worker.shutdown().expect("shutdown").incomplete());
}

#[test]
fn forged_context_owner_credit_replay_and_wrong_socket_close_only_the_branch() {
    for case in 0..5 {
        let (worker, mut host, _anchor, client) = setup();
        let producer = client
            .0
            .credits
            .lock()
            .expect("credits")
            .last()
            .expect("credit")
            .producer;
        let lineage = if case == 0 {
            Lineage::root(id(2), id(99))
        } else {
            host.lineage
        };
        let owner = if case == 1 {
            Owner::Magi
        } else {
            Owner::Casper
        };
        let producer = if case == 2 { id(99) } else { producer };
        let (socket, _sender) = transport::pair().expect("channel");
        let (stream, _peer) = std::os::unix::net::UnixStream::pair().expect("wrong type");
        let socket = if case == 3 {
            OwnedFd::from(stream)
        } else {
            socket
        };
        if case == 4 {
            let opened = client.open(Owner::Casper, lineage).expect("first binding");
            assert_eq!(opened.sender.identity(), producer);
            assert!(host.poll());
            client
                .0
                .send(
                    Action::Bind {
                        producer,
                        program: owner,
                        lineage,
                    },
                    Some(&socket),
                )
                .expect("replay");
            assert!(!host.poll());
            drop(opened);
        } else {
            client
                .0
                .send(
                    Action::Bind {
                        producer,
                        program: owner,
                        lineage,
                    },
                    Some(&socket),
                )
                .expect("forged frame");
            assert!(!host.poll());
        }
        assert_eq!(
            worker.readout().snapshot().counts.refused_producers,
            1,
            "case {case}"
        );
        let unaffected = worker.open(Owner::Magi).expect("root remains useful");
        drop(unaffected);
        drop(client);
        assert!(worker.shutdown().expect("shutdown").incomplete());
    }
}

#[test]
fn foreign_context_and_busy_cache_do_not_consume_a_launch_credit() {
    let (worker, host, _anchor, client) = setup();
    assert!(matches!(
        client.open(Owner::Magi, host.lineage),
        Err(RegisterError::Lineage)
    ));
    assert!(matches!(
        client.open(Owner::Casper, Lineage::root(id(99), id(3))),
        Err(RegisterError::Lineage)
    ));
    let cache = client.0.credits.lock().expect("cache held");
    assert!(matches!(
        client.open(Owner::Casper, host.lineage),
        Err(RegisterError::Busy)
    ));
    assert_eq!(cache.len(), CREDITS);
    drop(cache);
    assert!(client.open(Owner::Casper, host.lineage).is_ok());
    drop(client);
    drop(host);
    worker.shutdown().expect("shutdown");
}
