#![allow(clippy::expect_used)]

use super::super::tests::{event, id, setup, wait};
use super::*;
use crate::events::{
    Loss,
    collection::{Report, Sent},
};
use std::time::{Duration, Instant};

#[test]
fn nested_credit_binds_queued_events_and_survives_immediate_parent_exit() {
    let (worker, mut host, mut anchor, client) = setup();
    let parent = Lineage {
        turn_id: Some(id(4)),
        ask_id: Some(id(5)),
        ..host.lineage
    };
    let branch = client.branch(parent).expect("nested admission");
    let (mut sender, control, frozen) = branch.into_parts();
    let lineage = sender.lineage();
    assert_eq!(frozen, parent);
    assert_eq!(lineage.run_id, parent.run_id);
    assert_ne!(lineage.agent_id, parent.agent_id);
    assert_ne!(lineage.agent_id, Some(sender.identity()));
    assert!(lineage.turn_id.is_none() && lineage.ask_id.is_none());
    let nested = Client::adopt(control, lineage).expect("nested client");
    let bytes = serde_json::to_vec(&serde_json::json!({
        "schema":1, "program":"magi", "program_version":[1,0,0],
        "producer":sender.identity(), "sequence":1, "correlation":lineage,
        "fact":{"kind":"agent_started", "parent":parent}
    }))
    .expect("agent fact");
    assert_eq!(sender.send(&bytes), Sent::Queued);
    assert_eq!(worker.readout().snapshot().remote_producers, 1);
    assert_eq!(worker.readout().snapshot().counts.accepted, 0);
    assert!(host.poll_branches(true));
    let mut child_host = host.take_branch().expect("root-owned nested branch");
    wait(&worker.readout(), |s| s.counts.accepted == 1);
    assert!(child_host.poll());
    nested.poll().expect("child credit");
    assert_eq!(
        anchor.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    drop(client);
    assert!(!host.poll());
    let mut native = nested
        .open(Owner::Casper, lineage)
        .expect("supplier after ancestor exit");
    assert_eq!(
        native.sender.send(&event(&native.sender, lineage)),
        Sent::Queued
    );
    assert_eq!(
        native.sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    assert!(child_host.poll());
    wait(&worker.readout(), |s| {
        s.counts.accepted == 2 && s.retired.final_confirmed == 2
    });
    drop(native);
    assert!(child_host.poll());
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    drop(nested);
    assert!(!child_host.poll());
    wait(&worker.readout(), |s| s.retired.final_confirmed == 3);
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.counts.invalid, 0);
    assert_eq!(summary.counts.refused_producers, 0);
    assert!(!summary.incomplete());
}

#[test]
fn stalled_nested_admission_is_nonblocking_and_unadmitted_credits_are_not_producers() {
    let (worker, mut host, _anchor, client) = setup();
    let first = client.branch(host.lineage).expect("first");
    let second = client.branch(host.lineage).expect("second");
    let start = Instant::now();
    for _ in 0..10_000 {
        assert!(matches!(
            client.branch(host.lineage),
            Err(RegisterError::Busy)
        ));
    }
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(worker.readout().snapshot().remote_producers, 1);
    assert!(host.poll_branches(false));
    assert!(host.take_branch().is_none());
    assert_eq!(worker.readout().snapshot().counts.refused_producers, 1);
    assert_eq!(worker.readout().snapshot().remote_producers, 1);
    drop(first);
    drop(second);
    drop(client);
    drop(host);
    worker.shutdown().expect("shutdown");
}

#[test]
fn queued_nested_registration_survives_parent_close_before_root_poll() {
    let (worker, mut host, _anchor, client) = setup();
    let branch = client
        .branch(host.lineage)
        .expect("queued nested admission");
    drop(client);
    assert!(!host.poll_branches(true));
    let mut admitted = host
        .take_branch()
        .expect("accepted child survives parent offer failure");
    let (mut sender, control, _) = branch.into_parts();
    let lineage = sender.lineage();
    let child = Client::adopt(control, lineage).expect("child remains connected to root");
    assert!(admitted.poll());
    child.poll().expect("root credit after parent exit");
    assert!(child.open(Owner::Casper, lineage).is_ok());
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    drop(child);
    while admitted.poll() {}
    wait(&worker.readout(), |s| s.retired.final_confirmed == 1);
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.counts.invalid, 0);
    assert_eq!(summary.counts.refused_producers, 0);
}

#[test]
fn nested_agents_and_direct_suppliers_share_the_same_global_capacity() {
    let (worker, mut host, _anchor, client) = setup();
    let registrar = worker.registrar();
    let mut direct = Vec::new();
    while registrar.available() {
        direct.push(registrar.open(Owner::Magi).expect("direct"));
    }
    assert_eq!(direct.len(), MAX_PRODUCERS - 2 - CREDITS);
    let first = client.branch(host.lineage).expect("first reserved credit");
    let second = client.branch(host.lineage).expect("second reserved credit");
    assert!(matches!(
        client.branch(host.lineage),
        Err(RegisterError::Busy)
    ));
    assert!(host.poll_branches(true));
    let first_host = host.take_branch().expect("first host");
    assert!(host.poll_branches(true));
    let second_host = host.take_branch().expect("second host");
    assert!(!registrar.available());
    assert_eq!(
        worker.readout().snapshot().remote_producers,
        MAX_PRODUCERS - 1
    );
    assert!(matches!(
        registrar.open(Owner::Magi),
        Err(RegisterError::Capacity)
    ));
    drop(first);
    drop(second);
    drop(first_host);
    drop(second_host);
    drop(client);
    drop(host);
    drop(direct);
    let summary = worker.shutdown().expect("shutdown");
    assert_eq!(summary.counts.invalid, 0);
    assert_eq!(summary.counts.refused_producers, 1);
}

#[test]
fn nested_foreign_parent_and_busy_cache_preserve_credits() {
    let (worker, host, _anchor, client) = setup();
    for parent in [Lineage::root(id(99), id(3)), Lineage::root(id(2), id(99))] {
        assert!(matches!(client.branch(parent), Err(RegisterError::Lineage)));
    }
    let cache = client.0.credits.lock().expect("cache");
    assert!(matches!(
        client.branch(host.lineage),
        Err(RegisterError::Busy)
    ));
    assert_eq!(cache.len(), CREDITS);
    drop(cache);
    drop(client);
    drop(host);
    worker.shutdown().expect("shutdown");
}

#[test]
fn forged_nested_parent_credit_socket_and_replay_close_only_requesting_branch() {
    for case in 0..5 {
        let (worker, mut host, _anchor, client) = setup();
        let grant = *client
            .0
            .credits
            .lock()
            .expect("credits")
            .last()
            .expect("grant");
        let (events, _sender) = transport::pair().expect("events");
        let (control, _peer) = wire::endpoints().expect("control");
        let parent = if case == 0 {
            Lineage::root(id(2), id(99))
        } else {
            host.lineage
        };
        let producer = if case == 1 { id(99) } else { grant.producer };
        let mut admitted = None;
        let mut branch = None;
        if case == 4 {
            branch = Some(client.branch(parent).expect("initial branch"));
            assert!(host.poll_branches(true));
            admitted = host.take_branch();
            assert!(admitted.is_some());
        }
        let descriptors = match case {
            2 => [control.as_fd(), events.as_fd()],
            3 => [events.as_fd(), events.as_fd()],
            _ => [events.as_fd(), control.as_fd()],
        };
        client
            .0
            .socket
            .send_descriptors(Action::Branch { producer, parent }, &descriptors)
            .expect("forged branch");
        assert!(!host.poll_branches(true));
        assert_eq!(
            worker.readout().snapshot().counts.refused_producers,
            1,
            "case {case}"
        );
        drop(worker.open(Owner::Magi).expect("root remains useful"));
        drop(admitted);
        drop(branch);
        drop(client);
        worker.shutdown().expect("shutdown");
    }
}
