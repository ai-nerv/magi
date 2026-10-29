#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use rustix::{
    fd::AsRawFd,
    io::{FdFlags, fcntl_getfd},
};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn pair_is_private_bounded_nonblocking_and_close_on_exec() {
    let (channel, sender) = Channel::pair().expect("pair");
    for socket in [&channel.socket, &sender.socket] {
        assert!(
            fcntl_getfd(socket)
                .expect("descriptor flags")
                .contains(FdFlags::CLOEXEC)
        );
        assert!(
            rustix::fs::fcntl_getfl(socket)
                .expect("file flags")
                .contains(rustix::fs::OFlags::NONBLOCK)
        );
        assert!(
            sockopt::get_socket_recv_buffer_size(socket).expect("receive buffer") <= MAX_BUFFER
        );
        assert!(sockopt::get_socket_send_buffer_size(socket).expect("send buffer") <= MAX_BUFFER);
        let net::SocketAddrAny::Unix(address) = net::getsockname(socket).expect("address") else {
            panic!("private Unix channel");
        };
        assert!(address.path().is_none());
    }
    assert_eq!(channel.receive(&mut [0; MAX_EVENT_BYTES]), Received::Empty);
}

#[test]
fn whole_datagram_boundary_and_oversize_rejection_preserve_next_frame() {
    let (channel, mut sender) = Channel::pair().expect("pair");
    let mut bytes = [0; MAX_EVENT_BYTES];
    assert_eq!(sender.send(&[b'x'; MAX_EVENT_BYTES]), Sent::Queued);
    assert_eq!(
        channel.receive(&mut bytes),
        Received::Frame(MAX_EVENT_BYTES)
    );
    assert_eq!(bytes, [b'x'; MAX_EVENT_BYTES]);
    assert_eq!(sender.send(&[]), Sent::Invalid);
    assert_eq!(sender.send(&[b'x'; MAX_EVENT_BYTES + 1]), Sent::Invalid);
    net::send(
        &sender.socket,
        &[b'x'; MAX_EVENT_BYTES + 1],
        SendFlags::DONTWAIT,
    )
    .expect("hostile frame");
    assert_eq!(channel.receive(&mut bytes), Received::Invalid);
    assert_eq!(sender.send(b"next"), Sent::Queued);
    assert_eq!(channel.receive(&mut bytes), Received::Frame(4));
    assert_eq!(&bytes[..4], b"next");
    assert_eq!(sender.loss().invalid, 2);
}

#[test]
fn ancillary_descriptors_are_rejected_not_adopted() {
    let (channel, sender) = Channel::pair().expect("pair");
    use rustix::{
        fd::AsFd,
        net::{SendAncillaryBuffer, SendAncillaryMessage},
    };
    let file = std::fs::File::open("/dev/null").expect("file");
    let descriptors = [file.as_fd()];
    let mut storage = [0; rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    assert!(control.push(SendAncillaryMessage::ScmRights(&descriptors)));
    net::sendmsg(
        &sender.socket,
        &[std::io::IoSlice::new(b"untrusted")],
        &mut control,
        SendFlags::DONTWAIT,
    )
    .expect("ancillary send");
    assert_eq!(
        channel.receive(&mut [0; MAX_EVENT_BYTES]),
        Received::Invalid
    );
}

#[test]
fn stalled_and_closed_readers_never_retry_or_block() {
    let (channel, mut sender) = Channel::pair().expect("pair");
    let start = Instant::now();
    let mut full = false;
    for _ in 0..10000 {
        match sender.send(&[b'x'; MAX_EVENT_BYTES]) {
            Sent::Queued => {}
            Sent::Full => {
                full = true;
                break;
            }
            outcome => panic!("unexpected {outcome:?}"),
        }
    }
    assert!(full);
    assert_eq!(sender.loss().full, 1);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(
        channel.receive(&mut [0; MAX_EVENT_BYTES]),
        Received::Frame(MAX_EVENT_BYTES)
    );
    drop(channel);
    assert_eq!(sender.send(b"closed"), Sent::Unavailable);
    assert_eq!(sender.loss().unavailable, 1);
}

#[test]
fn logical_receiver_close_shuts_down_temporarily_duplicated_descriptors() {
    let (channel, mut sender) = Channel::pair().expect("pair");
    let inherited = rustix::io::dup(&channel.socket).expect("inherited descriptor");
    drop(channel);
    assert_eq!(
        net::sockopt::get_socket_type(&inherited).expect("duplicate remains open"),
        net::SocketType::DGRAM
    );
    assert_eq!(sender.send(b"after logical close"), Sent::Unavailable);
    assert_eq!(sender.loss().unavailable, 1);
    let (next, mut writer) = Channel::pair().expect("new pair");
    drop(inherited);
    assert_eq!(writer.send(b"next"), Sent::Queued);
    assert_eq!(next.receive(&mut [0; MAX_EVENT_BYTES]), Received::Frame(4));
}

#[test]
fn native_exec_does_not_inherit_either_channel_endpoint() {
    use std::io::Read;
    const KEY: &str = "MAGI_EVENT_TEST_FD_TARGETS";
    if let Ok(targets) = std::env::var(KEY) {
        let targets: Vec<_> = targets.split(',').collect();
        for entry in std::fs::read_dir("/proc/self/fd").expect("descriptors") {
            let Ok(link) = std::fs::read_link(entry.expect("entry").path()) else {
                continue;
            };
            assert!(
                !targets
                    .iter()
                    .any(|target| link == std::path::Path::new(target))
            );
        }
        println!("descriptor exclusion checked");
        return;
    }
    let (channel, sender) = Channel::pair().expect("pair");
    let targets = [&channel.socket, &sender.socket]
        .iter()
        .map(|fd| {
            std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
                .expect("socket identity")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut child = Reaped(Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "events::collection::transport::tests::native_exec_does_not_inherit_either_channel_endpoint", "--nocapture"])
        .env(KEY, targets)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn child"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait().expect("wait") {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            panic!("descriptor child deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut output = String::new();
    child
        .0
        .stdout
        .take()
        .expect("capture")
        .take(4097)
        .read_to_string(&mut output)
        .expect("child output");
    assert!(output.len() <= 4096);
    assert!(output.contains("descriptor exclusion checked"));
    assert!(output.contains("test result: ok. 1 passed; 0 failed; 0 ignored;"));
}

struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
