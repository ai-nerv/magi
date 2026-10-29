#![allow(clippy::expect_used, clippy::indexing_slicing)]

pub(in crate::events::collection) mod native;

use super::super::{Collector, Opened, Report, Sent};
use super::*;
use crate::events::{Emitter, Loss};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const GOLDEN: &str = include_str!("golden.json");

#[test]
fn full_lineage_fits_the_packet_and_is_preserved_through_descriptor_transfer() {
    let (mut collector, _) = setup();
    let id = || Id::generate().expect("identity");
    let lineage = Lineage {
        turn_id: Some(id()),
        ask_id: Some(id()),
        ..Lineage::root(id(), id())
    };
    let opened = collector
        .open_bound(Owner::Magi, lineage)
        .expect("bound registration");
    let input = std::fs::File::open("/dev/null").expect("stdin");
    let packet = prepare(opened.sender, input).expect("bounded packet");
    let received = receive_from(packet, Owner::Magi, process::getpid()).expect("receive");
    assert_eq!(received.sender.lineage(), lineage);
    let mut bytes = [0; MAX_BOOTSTRAP_BYTES];
    assert!(encode(Owner::Balthasar, id(), lineage, &mut bytes).expect("longest owner") <= 512);
}

fn setup() -> (Collector, Opened) {
    let (_, receiver) = Emitter::new(Id::generate().expect("local identity"), [1, 2, 3]);
    let mut collector = Collector::new(receiver);
    let opened = collector.open(Owner::Magi).expect("registration");
    (collector, opened)
}

fn event(producer: Id) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema":1,"program":"magi","program_version":[1,2,3],
        "producer":String::from(producer),"sequence":1,"correlation":{},
        "fact":{"kind":"client","outcome":"connected"}
    }))
    .expect("event")
}

fn raw(packet: &[u8], fds: &[rustix::fd::BorrowedFd<'_>]) -> OwnedFd {
    let (parent, child) = pair().expect("bootstrap pair");
    let mut storage = [0; rustix::cmsg_space!(ScmRights(8))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    if !fds.is_empty() {
        assert!(control.push(SendAncillaryMessage::ScmRights(fds)));
    }
    assert_eq!(
        net::sendmsg(
            &parent,
            &[IoSlice::new(packet)],
            &mut control,
            SendFlags::DONTWAIT | SendFlags::NOSIGNAL
        )
        .expect("raw packet"),
        packet.len()
    );
    child
}

#[test]
fn bootstrap_golden_is_independent_and_exact() {
    let id = Id::try_from("00000000000000000000000000000002".to_owned()).expect("id");
    let mut bytes = [0; MAX_BOOTSTRAP_BYTES];
    let len = encode(Owner::Magi, id, Lineage::default(), &mut bytes).expect("encode");
    assert_eq!(&bytes[..len], GOLDEN.trim_end().as_bytes());
}

#[test]
fn fresh_descriptor_transfer_preserves_identity_stdin_and_final_accounting() {
    let (mut collector, Opened { sender, lifetime }) = setup();
    let identity = sender.identity();
    let (input, mut writer) = UnixStream::pair().expect("stdin");
    writer.write_all(b"native-input-canary").expect("input");
    writer
        .shutdown(std::net::Shutdown::Write)
        .expect("input eof");
    let bootstrap = prepare(sender, &input).expect("prepare");
    let Incoming {
        mut sender,
        input: received,
    } = receive_from(&bootstrap, Owner::Magi, process::getpid()).expect("receive");
    assert_eq!(sender.identity(), identity);
    assert!(
        rustix::io::fcntl_getfd(&received)
            .expect("flags")
            .contains(rustix::io::FdFlags::CLOEXEC)
    );
    let mut body = String::new();
    std::fs::File::from(received)
        .read_to_string(&mut body)
        .expect("stdin body");
    assert_eq!(body, "native-input-canary");
    assert_eq!(sender.send(&event(identity)), Sent::Queued);
    assert_eq!(
        sender.report(Loss::default(), true),
        Report::Sent(Sent::Queued)
    );
    collector.poll();
    collector.poll();
    drop(lifetime);
    assert_eq!(collector.counts().accepted, 1);
    let summary = collector.close();
    assert_eq!(summary.retired.final_confirmed, 1);
    assert!(!summary.incomplete());
}

#[test]
fn unavailable_bootstrap_never_waits_for_a_sender() {
    let (_parent, child) = pair().expect("pair");
    let now = Instant::now();
    assert!(matches!(
        receive_from(child, Owner::Magi, process::getpid()),
        Err(Error::Transport)
    ));
    assert!(now.elapsed() < Duration::from_secs(1));
}

#[test]
fn consumed_or_reported_senders_cannot_reset_sequence_accounting_through_handoff() {
    let input = std::fs::File::open("/dev/null").expect("input");
    for state in 0..3 {
        let (
            _collector,
            Opened {
                mut sender,
                lifetime,
            },
        ) = setup();
        match state {
            0 => {
                assert_eq!(sender.send(&event(sender.identity())), Sent::Queued);
            }
            1 => {
                assert_eq!(sender.send(b""), Sent::Invalid);
            }
            _ => {
                assert_eq!(
                    sender.report(Loss::default(), false),
                    Report::Sent(Sent::Queued)
                );
            }
        }
        assert!(matches!(prepare(sender, &input), Err(Error::UsedSender)));
        drop(lifetime);
    }
}

fn reject_without_leaking(packet: &[u8], count: usize, owner: Owner) {
    let (_collector, Opened { sender, lifetime }) = setup();
    let (sender, _, _) = sender.into_fresh().expect("fresh");
    let event = sender.into_socket();
    let (input, mut peer) = UnixStream::pair().expect("input");
    peer.set_nonblocking(true).expect("nonblocking sentinel");
    let mut fds = vec![event.as_fd()];
    for _ in 1..count {
        fds.push(input.as_fd());
    }
    fds.truncate(count);
    let child = raw(packet, &fds);
    drop(fds);
    drop(input);
    assert!(receive_from(&child, owner, process::getpid()).is_err());
    let mut byte = [0];
    assert_eq!(
        peer.read(&mut byte)
            .expect("all received input descriptors closed"),
        0
    );
    drop(lifetime);
}

#[test]
fn hostile_frames_drop_all_received_descriptors() {
    if native::isolate(
        "events::collection::bootstrap::tests::hostile_frames_drop_all_received_descriptors",
    ) {
        return;
    }
    let golden = GOLDEN.trim_end();
    let hostile = [
        golden.replace("\"schema\":2", "\"schema\":1"),
        golden.replace("\"schema\":2", "\"schema\":3"),
        golden.replace(
            "\"run_id\":null",
            "\"run_id\":\"00000000000000000000000000000001\"",
        ),
        golden.replace(
            "\"turn_id\":null",
            "\"turn_id\":\"00000000000000000000000000000003\"",
        ),
        golden.replace(
            "\"ask_id\":null",
            "\"ask_id\":\"00000000000000000000000000000004\"",
        ),
        golden.replace("\"schema\":2", "\"schema\":2,\"schema\":2"),
        golden.replace("native_event_bootstrap", "untrusted"),
        golden.replace("\"program\":\"magi\"", "\"program\":\"unknown\""),
        golden.replace("00000000000000000000000000000002", "private-value"),
        golden.replace("\"schema\":2", "\"secret\":\"private\",\"schema\":2"),
        "x".repeat(MAX_BOOTSTRAP_BYTES + 1),
    ];
    for packet in hostile {
        reject_without_leaking(packet.as_bytes(), 2, Owner::Magi);
    }
    reject_without_leaking(golden.as_bytes(), 2, Owner::Casper);
}

#[test]
fn descriptor_count_and_control_truncation_are_rejected_without_leaks() {
    if native::isolate(
        "events::collection::bootstrap::tests::descriptor_count_and_control_truncation_are_rejected_without_leaks",
    ) {
        return;
    }
    for count in [0, 1, 3, 8] {
        reject_without_leaking(GOLDEN.trim_end().as_bytes(), count, Owner::Magi);
    }
}

#[test]
fn leak_sentinel_distinguishes_received_rights_from_other_descriptor_references() {
    if native::isolate(
        "events::collection::bootstrap::tests::leak_sentinel_distinguishes_received_rights_from_other_descriptor_references",
    ) {
        return;
    }
    let (_collector, Opened { sender, lifetime }) = setup();
    let (sender, _, _) = sender.into_fresh().expect("fresh");
    let event = sender.into_socket();
    let (input, mut peer) = UnixStream::pair().expect("input");
    let inherited = input.try_clone().expect("temporary inherited reference");
    peer.set_nonblocking(true).expect("nonblocking sentinel");
    let child = raw(b"malformed", &[event.as_fd(), input.as_fd()]);
    drop(input);
    assert!(matches!(
        receive_from(&child, Owner::Magi, process::getpid()),
        Err(Error::Frame)
    ));
    assert_eq!(
        peer.read(&mut [0])
            .expect_err("other reference prevents EOF")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(inherited);
    assert_eq!(
        peer.read(&mut [0])
            .expect("received descriptor was already closed"),
        0
    );
    drop(lifetime);
}

#[test]
fn wrong_parent_is_rejected_before_receiving_capabilities() {
    let (_collector, Opened { sender, lifetime }) = setup();
    let input = std::fs::File::open("/dev/null").expect("stdin");
    let child = prepare(sender, input).expect("prepare");
    let wrong = Pid::from_raw(process::getpid().as_raw_nonzero().get() + 1).expect("other pid");
    assert!(matches!(
        receive_from(&child, Owner::Magi, wrong),
        Err(Error::Peer)
    ));
    assert!(receive_from(&child, Owner::Magi, process::getpid()).is_ok());
    drop(lifetime);
}

#[test]
fn input_and_event_descriptors_cannot_be_swapped_or_used_as_capability_stdin() {
    let (_collector, Opened { sender, lifetime }) = setup();
    let (sender, _, _) = sender.into_fresh().expect("fresh");
    let event = sender.into_socket();
    let input = std::fs::File::open("/dev/null").expect("input");
    for fds in [
        [input.as_fd(), event.as_fd()],
        [event.as_fd(), event.as_fd()],
    ] {
        let child = raw(GOLDEN.trim_end().as_bytes(), &fds);
        assert!(matches!(
            receive_from(child, Owner::Magi, process::getpid()),
            Err(Error::Descriptor)
        ));
    }
    let output = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .expect("output");
    let child = raw(
        GOLDEN.trim_end().as_bytes(),
        &[event.as_fd(), output.as_fd()],
    );
    assert!(matches!(
        receive_from(child, Owner::Magi, process::getpid()),
        Err(Error::Descriptor)
    ));
    drop(lifetime);
}

#[test]
fn blocking_or_oversized_event_socket_is_not_adopted() {
    let input = std::fs::File::open("/dev/null").expect("input");
    for oversized in [false, true] {
        let (_collector, Opened { sender, lifetime }) = setup();
        let (sender, _, _) = sender.into_fresh().expect("fresh");
        let event = sender.into_socket();
        if oversized {
            sockopt::set_socket_send_buffer_size(&event, 256 * 1024).expect("large buffer");
            assert!(sockopt::get_socket_send_buffer_size(&event).expect("buffer") > 64 * 1024);
        } else {
            let flags = rustix::fs::fcntl_getfl(&event).expect("flags");
            rustix::fs::fcntl_setfl(&event, flags & !rustix::fs::OFlags::NONBLOCK)
                .expect("blocking");
        }
        let child = raw(
            GOLDEN.trim_end().as_bytes(),
            &[event.as_fd(), input.as_fd()],
        );
        assert!(matches!(
            receive_from(child, Owner::Magi, process::getpid()),
            Err(Error::Descriptor)
        ));
        drop(lifetime);
    }
}

#[test]
fn unnamed_validation_does_not_accept_named_or_empty_abstract_addresses() {
    use std::os::linux::net::SocketAddrExt;
    assert!(
        !std::os::unix::net::SocketAddr::from_abstract_name(b"")
            .expect("empty abstract address")
            .is_unnamed()
    );
    let (_collector, Opened { sender, lifetime }) = setup();
    let (sender, _, _) = sender.into_fresh().expect("fresh");
    let event = sender.into_socket();
    private_socket(&event, SocketType::DGRAM).expect("unnamed pair");
    let name = format!(
        "magi-events-test-{}",
        String::from(Id::generate().expect("name"))
    );
    let address = net::SocketAddrUnix::new_abstract_name(name.as_bytes()).expect("named abstract");
    net::bind_unix(&event, &address).expect("bind existing socket");
    assert_eq!(
        private_socket(&event, SocketType::DGRAM),
        Err(Error::Descriptor)
    );
    drop(lifetime);
}
