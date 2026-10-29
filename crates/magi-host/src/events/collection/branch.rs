use super::{
    Owner, Sender,
    bootstrap::{self, Error},
    transport,
};
use crate::events::{Id, Lineage, Revision};
use rustix::{
    fd::{AsFd, OwnedFd},
    net::{
        self, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
        SendAncillaryMessage, SendFlags, SocketType, sockopt,
    },
};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, IoSlice, IoSliceMut};

const LIMIT: usize = 512;

#[derive(Serialize, Deserialize)]
enum Kind {
    #[serde(rename = "agent_event_bootstrap")]
    Agent,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    schema: Revision,
    kind: Kind,
    producer: Id,
    lineage: Lineage,
    parent: Lineage,
}

/// A fresh child producer, root enrollment endpoint and immutable parent context.
pub struct Branch {
    sender: Sender,
    control: OwnedFd,
    parent: Lineage,
}

impl Branch {
    pub(super) fn new(sender: Sender, control: OwnedFd, parent: Lineage) -> Result<Self, Error> {
        validate(sender.lineage(), parent)?;
        Ok(Self {
            sender,
            control,
            parent,
        })
    }

    pub fn into_parts(self) -> (Sender, OwnedFd, Lineage) {
        (self.sender, self.control, self.parent)
    }

    /// Create a new immediate-parent packet for the next native process hop.
    pub fn prepare(self, input: impl AsFd) -> Result<OwnedFd, Error> {
        bootstrap::check_input(&input)?;
        let lineage = self.sender.lineage();
        validate(lineage, self.parent)?;
        let (sender, owner, producer) = self.sender.into_fresh().ok_or(Error::UsedSender)?;
        if owner != Owner::Magi {
            return Err(Error::Frame);
        }
        transport::private_socket(&self.control, SocketType::SEQPACKET)
            .map_err(|_| Error::Descriptor)?;
        let control = transport::Sender::adopt(self.control)
            .map_err(|_| Error::Descriptor)?
            .into_socket();
        let event = sender.into_socket();
        let (root, child) = bootstrap::pair()?;
        let mut bytes = [0; LIMIT];
        let mut cursor = Cursor::new(bytes.as_mut_slice());
        serde_json::to_writer(
            &mut cursor,
            &Frame {
                schema: Revision,
                kind: Kind::Agent,
                producer,
                lineage,
                parent: self.parent,
            },
        )
        .map_err(|_| Error::Frame)?;
        let len = usize::try_from(cursor.position()).map_err(|_| Error::Frame)?;
        let rights = [event.as_fd(), control.as_fd(), input.as_fd()];
        let mut storage = [0; rustix::cmsg_space!(ScmRights(3))];
        let mut ancillary = SendAncillaryBuffer::new(&mut storage);
        if !ancillary.push(SendAncillaryMessage::ScmRights(&rights)) {
            return Err(Error::Ancillary);
        }
        let bytes = bytes.get(..len).ok_or(Error::Frame)?;
        let n = net::sendmsg(
            &root,
            &[IoSlice::new(bytes)],
            &mut ancillary,
            SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
        )
        .map_err(|_| Error::Transport)?;
        if n != len {
            return Err(Error::Transport);
        }
        Ok(child)
    }

    /// Validate the immediate native parent and restore stdin before normal work.
    pub fn receive_stdin() -> Result<Self, Error> {
        let parent = rustix::process::getppid().ok_or(Error::Peer)?;
        let (branch, original) = Self::receive_from(std::io::stdin(), parent)?;
        rustix::stdio::dup2_stdin(&original).map_err(|_| Error::Descriptor)?;
        Ok(branch)
    }

    fn receive_from(
        input: impl AsFd,
        parent: rustix::process::Pid,
    ) -> Result<(Self, OwnedFd), Error> {
        transport::private_socket(&input, SocketType::SEQPACKET).map_err(|_| Error::Descriptor)?;
        if sockopt::get_socket_peercred(&input)
            .map_err(|_| Error::Peer)?
            .pid
            != parent
        {
            return Err(Error::Peer);
        }
        let mut bytes = [0; LIMIT];
        let mut storage = [0; rustix::cmsg_space!(ScmRights(3))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut storage);
        let reply = net::recvmsg(
            &input,
            &mut [IoSliceMut::new(&mut bytes)],
            &mut ancillary,
            RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
        )
        .map_err(|_| Error::Transport)?;
        let mut descriptors = Vec::with_capacity(3);
        for message in ancillary.drain() {
            let RecvAncillaryMessage::ScmRights(rights) = message else {
                return Err(Error::Ancillary);
            };
            for fd in rights {
                if descriptors.len() == 3 {
                    return Err(Error::Ancillary);
                }
                descriptors.push(fd);
            }
        }
        if reply.bytes == 0 || !reply.flags.difference(RecvFlags::CMSG_CLOEXEC).is_empty() {
            return Err(Error::Frame);
        }
        let frame: Frame = serde_json::from_slice(bytes.get(..reply.bytes).ok_or(Error::Frame)?)
            .map_err(|_| Error::Frame)?;
        validate(frame.lineage, frame.parent)?;
        let [event, control, original]: [OwnedFd; 3] =
            descriptors.try_into().map_err(|_| Error::Ancillary)?;
        transport::private_socket(&event, SocketType::DGRAM).map_err(|_| Error::Descriptor)?;
        transport::private_socket(&control, SocketType::SEQPACKET)
            .map_err(|_| Error::Descriptor)?;
        let event = transport::Sender::adopt(event).map_err(|_| Error::Descriptor)?;
        let control = transport::Sender::adopt(control)
            .map_err(|_| Error::Descriptor)?
            .into_socket();
        bootstrap::check_input(&original)?;
        Ok((
            Self {
                sender: Sender::new(event, Owner::Magi, frame.producer).with_lineage(frame.lineage),
                control,
                parent: frame.parent,
            },
            original,
        ))
    }
}

fn validate(lineage: Lineage, parent: Lineage) -> Result<(), Error> {
    if !lineage.valid()
        || !parent.valid()
        || lineage.run_id.is_none()
        || lineage.run_id != parent.run_id
        || lineage.agent_id == parent.agent_id
        || lineage.turn_id.is_some()
        || lineage.ask_id.is_some()
    {
        return Err(Error::Frame);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
