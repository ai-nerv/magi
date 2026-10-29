#![allow(clippy::expect_used)]

use super::*;
use std::os::fd::AsRawFd;

fn copies(fd: &OwnedFd) -> usize {
    let target =
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd())).expect("descriptor");
    std::fs::read_dir("/proc/self/fd")
        .expect("descriptors")
        .filter_map(Result::ok)
        .filter(|entry| std::fs::read_link(entry.path()).ok().as_ref() == Some(&target))
        .count()
}

#[test]
fn malformed_control_packets_close_received_rights_without_leaking_descriptors() {
    for bytes in [
        b"not-json".as_slice(),
        br#"{"schema":2,"action":{"kind":"retire","producer":"00000000000000000000000000000001"}}"#,
        br#"{"schema":1,"action":{"kind":"retire","producer":"00000000000000000000000000000001","extra":true}}"#,
        br#"{"schema":1,"action":{"kind":"retire","producer":"00000000000000000000000000000001"}}"#,
        &[b'x'; LIMIT + 1],
    ] {
        let (root, child) = pair().expect("pair");
        let (descriptor, _sender) = transport::pair().expect("descriptor");
        let before = copies(&descriptor);
        let rights = [descriptor.as_fd()];
        let mut storage = [0; rustix::cmsg_space!(ScmRights(1))];
        let mut control = SendAncillaryBuffer::new(&mut storage);
        assert!(control.push(SendAncillaryMessage::ScmRights(&rights)));
        net::sendmsg(&child, &[IoSlice::new(bytes)], &mut control, SendFlags::DONTWAIT).expect("raw packet");
        assert!(root.receive().is_err());
        assert_eq!(copies(&descriptor), before);
    }
}

#[test]
fn missing_or_excess_rights_and_non_private_endpoints_are_refused() {
    let id = Id::try_from("00000000000000000000000000000001".to_owned()).expect("id");
    let lineage = Lineage::root(id, id);
    let (root, child) = pair().expect("pair");
    let peer = Socket::adopt(child).expect("client");
    peer.send(
        Action::Bind {
            producer: id,
            program: Owner::Casper,
            lineage,
        },
        None,
    )
    .expect("missing fd");
    assert!(root.receive().is_err());
    let (descriptor, _sender) = transport::pair().expect("descriptor");
    let before = copies(&descriptor);
    let rights = [descriptor.as_fd(), descriptor.as_fd()];
    let mut storage = [0; rustix::cmsg_space!(ScmRights(2))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    assert!(control.push(SendAncillaryMessage::ScmRights(&rights)));
    let bytes = serde_json::to_vec(&Frame {
        schema: Revision,
        action: Action::Bind {
            producer: id,
            program: Owner::Casper,
            lineage,
        },
    })
    .expect("frame");
    net::sendmsg(
        &peer.0,
        &[IoSlice::new(&bytes)],
        &mut control,
        SendFlags::DONTWAIT,
    )
    .expect("excess fds");
    assert!(root.receive().is_err());
    assert_eq!(copies(&descriptor), before);
    assert!(Socket::adopt(descriptor).is_err());
    let file = std::fs::File::open("/dev/null").expect("file");
    assert!(Socket::adopt(file.into()).is_err());
}

#[test]
fn control_backpressure_is_bounded_nonblocking_and_close_is_observable() {
    let (root, child) = pair().expect("pair");
    let peer = Socket::adopt(child).expect("client");
    let id = Id::try_from("00000000000000000000000000000001".to_owned()).expect("id");
    let start = std::time::Instant::now();
    let mut filled = false;
    for _ in 0..2048 {
        if let Err(error) = root.send(
            Action::Offer {
                producer: id,
                agent: id,
            },
            None,
        ) {
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            filled = true;
            break;
        }
    }
    assert!(filled && start.elapsed() < std::time::Duration::from_secs(1));
    peer.close();
    assert!(root.receive().is_err());
}

#[test]
fn nested_frames_require_exactly_two_close_on_exec_rights_without_leaks() {
    let id = Id::try_from("00000000000000000000000000000001".to_owned()).expect("id");
    let bytes = serde_json::to_vec(&Frame {
        schema: Revision,
        action: Action::Branch {
            producer: id,
            parent: Lineage::root(id, id),
        },
    })
    .expect("branch frame");
    for count in 0..=3 {
        let (root, child) = pair().expect("pair");
        let (descriptor, _sender) = transport::pair().expect("descriptor");
        let before = copies(&descriptor);
        let rights = [descriptor.as_fd(); 3];
        let mut storage = [0; rustix::cmsg_space!(ScmRights(3))];
        let mut control = SendAncillaryBuffer::new(&mut storage);
        if count != 0 {
            assert!(control.push(SendAncillaryMessage::ScmRights(
                rights.get(..count).expect("rights")
            )));
        }
        net::sendmsg(
            &child,
            &[IoSlice::new(&bytes)],
            &mut control,
            SendFlags::DONTWAIT,
        )
        .expect("packet");
        let received = root.receive();
        if count == 2 {
            let (_, descriptors) = received.expect("valid count").expect("packet");
            assert_eq!(descriptors.len(), 2);
            for fd in &descriptors {
                assert!(
                    rustix::io::fcntl_getfd(fd)
                        .expect("flags")
                        .contains(rustix::io::FdFlags::CLOEXEC)
                );
            }
        } else {
            assert!(received.is_err(), "count {count}");
        }
        assert_eq!(copies(&descriptor), before, "count {count}");
    }
}
