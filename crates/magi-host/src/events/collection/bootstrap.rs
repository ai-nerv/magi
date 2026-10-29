use super::{Id, Owner, Sender, transport};
use crate::events::Lineage;
use rustix::{
    fd::{AsFd, OwnedFd},
    net::{
        self, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
        SendAncillaryMessage, SendFlags, SocketType, sockopt,
    },
    process::{self, Pid},
};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, IoSlice, IoSliceMut};

pub const MAX_BOOTSTRAP_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("event sender already used")]
    UsedSender,
    #[error("invalid event bootstrap descriptor")]
    Descriptor,
    #[error("invalid event bootstrap parent")]
    Peer,
    #[error("invalid event bootstrap frame")]
    Frame,
    #[error("invalid event bootstrap descriptor count")]
    Ancillary,
    #[error("event bootstrap transport unavailable")]
    Transport,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    NativeEventBootstrap,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    schema: u8,
    kind: Kind,
    program: Owner,
    producer: Id,
    lineage: Lineage,
}

/// Prequeue one bootstrap packet; pass the returned descriptor as native child stdin.
pub fn prepare(sender: Sender, input: impl AsFd) -> Result<OwnedFd, Error> {
    let lineage = sender.lineage();
    if !lineage.valid() {
        return Err(Error::Frame);
    }
    check_input(&input)?;
    let (sender, owner, producer) = sender.into_fresh().ok_or(Error::UsedSender)?;
    let socket = sender.into_socket();
    let (parent, child) = pair()?;
    let mut bytes = [0; MAX_BOOTSTRAP_BYTES];
    let len = encode(owner, producer, lineage, &mut bytes)?;
    let descriptors = [socket.as_fd(), input.as_fd()];
    let mut storage = [0; rustix::cmsg_space!(ScmRights(2))];
    let mut control = SendAncillaryBuffer::new(&mut storage);
    if !control.push(SendAncillaryMessage::ScmRights(&descriptors)) {
        return Err(Error::Ancillary);
    }
    let packet = bytes.get(..len).ok_or(Error::Frame)?;
    let sent = net::sendmsg(
        &parent,
        &[IoSlice::new(packet)],
        &mut control,
        SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
    )
    .map_err(|_| Error::Transport)?;
    if sent != len {
        return Err(Error::Transport);
    }
    Ok(child)
}

/// Receive from the native parent and restore stdin before readers or children start.
pub fn receive_stdin(owner: Owner) -> Result<Sender, Error> {
    let parent = process::getppid().ok_or(Error::Peer)?;
    let incoming = receive_from(std::io::stdin(), owner, parent)?;
    rustix::stdio::dup2_stdin(&incoming.input).map_err(|_| Error::Descriptor)?;
    Ok(incoming.sender)
}

struct Incoming {
    sender: Sender,
    input: OwnedFd,
}

fn pair() -> Result<(OwnedFd, OwnedFd), Error> {
    let (parent, child) = net::socketpair(
        net::AddressFamily::UNIX,
        SocketType::SEQPACKET,
        net::SocketFlags::CLOEXEC | net::SocketFlags::NONBLOCK,
        None,
    )
    .map_err(|_| Error::Transport)?;
    transport::configure(&parent).map_err(|_| Error::Transport)?;
    transport::configure(&child).map_err(|_| Error::Transport)?;
    Ok((parent, child))
}

fn encode(
    owner: Owner,
    producer: Id,
    lineage: Lineage,
    bytes: &mut [u8; MAX_BOOTSTRAP_BYTES],
) -> Result<usize, Error> {
    let mut cursor = Cursor::new(bytes.as_mut_slice());
    serde_json::to_writer(
        &mut cursor,
        &Frame {
            schema: 2,
            kind: Kind::NativeEventBootstrap,
            program: owner,
            producer,
            lineage,
        },
    )
    .map_err(|_| Error::Frame)?;
    usize::try_from(cursor.position()).map_err(|_| Error::Frame)
}

fn receive_from(socket: impl AsFd, owner: Owner, parent: Pid) -> Result<Incoming, Error> {
    private_socket(&socket, SocketType::SEQPACKET)?;
    let peer = sockopt::get_socket_peercred(&socket).map_err(|_| Error::Peer)?;
    if peer.pid != parent || peer.uid != process::geteuid() {
        return Err(Error::Peer);
    }
    let mut bytes = [0; MAX_BOOTSTRAP_BYTES];
    let mut storage = [0; rustix::cmsg_space!(ScmRights(2))];
    let mut control = RecvAncillaryBuffer::new(&mut storage);
    let reply = net::recvmsg(
        &socket,
        &mut [IoSliceMut::new(&mut bytes)],
        &mut control,
        RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
    )
    .map_err(|_| Error::Transport)?;
    if !reply.flags.difference(RecvFlags::CMSG_CLOEXEC).is_empty() || reply.bytes == 0 {
        return Err(Error::Frame);
    }
    let frame: Frame = serde_json::from_slice(bytes.get(..reply.bytes).ok_or(Error::Frame)?)
        .map_err(|_| Error::Frame)?;
    if frame.schema != 2 || frame.program != owner || !frame.lineage.valid() {
        return Err(Error::Frame);
    }
    let [event, input] = descriptors(&mut control)?;
    private_socket(&event, SocketType::DGRAM)?;
    let event_peer = sockopt::get_socket_peercred(&event).map_err(|_| Error::Peer)?;
    if event_peer.uid != process::geteuid() {
        return Err(Error::Peer);
    }
    check_input(&input)?;
    let sender = transport::Sender::adopt(event).map_err(|_| Error::Descriptor)?;
    Ok(Incoming {
        sender: Sender::new(sender, owner, frame.producer).with_lineage(frame.lineage),
        input,
    })
}

fn descriptors(control: &mut RecvAncillaryBuffer<'_>) -> Result<[OwnedFd; 2], Error> {
    let mut found = [None, None];
    let mut count = 0;
    for message in control.drain() {
        let RecvAncillaryMessage::ScmRights(descriptors) = message else {
            return Err(Error::Ancillary);
        };
        for fd in descriptors {
            let slot = found.get_mut(count).ok_or(Error::Ancillary)?;
            *slot = Some(fd);
            count += 1;
        }
    }
    let [event, input] = found;
    Ok([
        event.ok_or(Error::Ancillary)?,
        input.ok_or(Error::Ancillary)?,
    ])
}

fn private_socket(fd: impl AsFd, kind: SocketType) -> Result<(), Error> {
    if sockopt::get_socket_type(&fd).map_err(|_| Error::Descriptor)? != kind
        || sockopt::get_socket_domain(&fd).map_err(|_| Error::Descriptor)?
            != net::AddressFamily::UNIX
    {
        return Err(Error::Descriptor);
    }
    let duplicate = fd
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| Error::Descriptor)?;
    let socket = std::os::unix::net::UnixDatagram::from(duplicate);
    if !socket
        .local_addr()
        .map_err(|_| Error::Descriptor)?
        .is_unnamed()
        || !socket
            .peer_addr()
            .map_err(|_| Error::Descriptor)?
            .is_unnamed()
    {
        return Err(Error::Descriptor);
    }
    Ok(())
}

fn check_input(input: impl AsFd) -> Result<(), Error> {
    let flags = rustix::fs::fcntl_getfl(&input).map_err(|_| Error::Descriptor)?;
    if flags.contains(rustix::fs::OFlags::WRONLY) {
        return Err(Error::Descriptor);
    }
    match sockopt::get_socket_type(input) {
        Ok(SocketType::STREAM) | Err(rustix::io::Errno::NOTSOCK) => Ok(()),
        _ => Err(Error::Descriptor),
    }
}

#[cfg(test)]
pub(super) mod tests;
