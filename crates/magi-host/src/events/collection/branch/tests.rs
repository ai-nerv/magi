#![allow(clippy::expect_used)]

use super::*;
use crate::events::{
    Emitter,
    collection::{Collector, Worker, enrollment::Host},
};

mod native;

fn id(n: u64) -> Id {
    Id::try_from(format!("{n:032x}")).expect("id")
}

fn setup() -> (Worker, Host, Branch) {
    let (_, receiver) = Emitter::new(id(1), [1, 0, 0]);
    let worker = Worker::start(Collector::new(receiver)).expect("worker");
    let parent = Lineage {
        turn_id: Some(id(4)),
        ask_id: Some(id(5)),
        ..Lineage::root(id(2), id(3))
    };
    let (host, sender, control) =
        Host::pair(worker.registrar(), Lineage::root(id(2), id(6))).expect("host");
    let branch = Branch::new(sender, control, parent).expect("branch");
    (worker, host, branch)
}

fn input() -> std::fs::File {
    std::fs::File::open("/dev/null").expect("input")
}

#[test]
fn fresh_packets_preserve_frozen_parent_and_independent_child_bindings() {
    let (worker, host, branch) = setup();
    let parent = branch.parent;
    let lineage = branch.sender.lineage();
    let producer = branch.sender.identity();
    let packet = branch.prepare(input()).expect("first hop");
    let (received, original) =
        Branch::receive_from(packet, rustix::process::getpid()).expect("receive");
    assert_eq!(received.parent, parent);
    assert_eq!(received.sender.lineage(), lineage);
    assert_eq!(received.sender.identity(), producer);
    assert!(lineage.turn_id.is_none() && lineage.ask_id.is_none());
    assert_ne!(lineage.agent_id, parent.agent_id);
    let packet = received.prepare(original).expect("fresh second hop");
    let (received, original) =
        Branch::receive_from(packet, rustix::process::getpid()).expect("second receive");
    assert_eq!(received.parent, parent);
    assert_eq!(received.sender.lineage(), lineage);
    assert!(
        rustix::io::fcntl_getfd(&received.control)
            .expect("flags")
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    let mut file = std::fs::File::from(original);
    assert_eq!(
        std::io::Read::read(&mut file, &mut [0; 1]).expect("original stdin"),
        0
    );
    drop(received);
    drop(host);
    worker.shutdown().expect("shutdown");
}

#[test]
fn wrong_parent_and_non_child_lineage_cannot_activate_a_branch() {
    let (worker, host, branch) = setup();
    let parent = branch.parent;
    let lineage = branch.sender.lineage();
    for invalid in [
        parent,
        Lineage::default(),
        Lineage::root(id(99), id(6)),
        Lineage {
            turn_id: Some(id(4)),
            ..lineage
        },
        Lineage {
            ask_id: Some(id(5)),
            ..lineage
        },
    ] {
        assert!(validate(invalid, parent).is_err());
    }
    let packet = branch.prepare(input()).expect("packet");
    assert!(matches!(
        Branch::receive_from(packet, rustix::process::getppid().expect("parent")),
        Err(Error::Peer)
    ));
    drop(host);
    worker.shutdown().expect("shutdown");
}

#[test]
fn malformed_child_packets_reject_revision_fields_and_descriptor_counts() {
    for case in 0..5 {
        let (worker, host, branch) = setup();
        let frame = serde_json::json!({"schema":1, "kind":"agent_event_bootstrap",
            "producer":branch.sender.identity(), "lineage":branch.sender.lineage(), "parent":branch.parent});
        let mut frame = frame;
        if case == 0 {
            frame["schema"] = 2.into();
        }
        if case == 1 {
            frame["extra"] = true.into();
        }
        let mut bytes = serde_json::to_vec(&frame).expect("frame");
        assert!(bytes.len() <= LIMIT);
        if case == 2 {
            bytes.resize(LIMIT + 1, b' ');
        }
        let (event, _, _) = branch.sender.into_fresh().expect("fresh");
        let event = event.into_socket();
        let original = input();
        let mut rights = vec![event.as_fd(), branch.control.as_fd(), original.as_fd()];
        if case == 3 {
            rights.pop();
        }
        if case == 4 {
            rights.push(original.as_fd());
        }
        let (root, packet) = bootstrap::pair().expect("packet pair");
        let mut storage = [0; rustix::cmsg_space!(ScmRights(4))];
        let mut control = SendAncillaryBuffer::new(&mut storage);
        assert!(control.push(SendAncillaryMessage::ScmRights(&rights)));
        net::sendmsg(
            &root,
            &[IoSlice::new(&bytes)],
            &mut control,
            SendFlags::DONTWAIT,
        )
        .expect("raw packet");
        assert!(
            Branch::receive_from(packet, rustix::process::getpid()).is_err(),
            "case {case}"
        );
        drop(host);
        worker.shutdown().expect("shutdown");
    }
}
