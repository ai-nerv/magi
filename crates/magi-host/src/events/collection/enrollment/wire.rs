use super::super::{Owner, transport};
use crate::events::{Id, Lineage, Revision};
use rustix::{
    fd::{AsFd, BorrowedFd, OwnedFd},
    net::{
        self, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
        SendAncillaryMessage, SendFlags, SocketType,
    },
};
use serde::{Deserialize, Serialize};
use std::io::{self, Cursor, IoSlice, IoSliceMut};

const LIMIT: usize = 512;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Offer {
        producer: Id,
        agent: Id,
    },
    Bind {
        producer: Id,
        program: Owner,
        lineage: Lineage,
    },
    Retire {
        producer: Id,
    },
    Branch {
        producer: Id,
        parent: Lineage,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    schema: Revision,
    action: Action,
}

pub(super) struct Socket(OwnedFd);

pub(super) fn pair() -> io::Result<(Socket, OwnedFd)> {
    let (root, child) = endpoints()?;
    Ok((Socket(root), child))
}

pub(super) fn endpoints() -> io::Result<(OwnedFd, OwnedFd)> {
    let (root, child) = net::socketpair(
        net::AddressFamily::UNIX,
        SocketType::SEQPACKET,
        net::SocketFlags::CLOEXEC | net::SocketFlags::NONBLOCK,
        None,
    )?;
    transport::configure(&root)?;
    transport::configure(&child)?;
    Ok((root, child))
}

impl Socket {
    pub(super) fn adopt(socket: OwnedFd) -> io::Result<Self> {
        transport::private_socket(&socket, SocketType::SEQPACKET)?;
        let checked = transport::Sender::adopt(socket)?;
        Ok(Self(checked.into_socket()))
    }

    pub(super) fn close(&self) {
        let _ = net::shutdown(&self.0, net::Shutdown::ReadWrite);
    }

    pub(super) fn send(&self, action: Action, descriptor: Option<&OwnedFd>) -> io::Result<()> {
        let descriptors = descriptor.map(|fd| [fd.as_fd()]);
        self.send_descriptors(
            action,
            descriptors.as_ref().map_or(&[], |fds| fds.as_slice()),
        )
    }

    pub(super) fn send_descriptors(
        &self,
        action: Action,
        descriptors: &[BorrowedFd<'_>],
    ) -> io::Result<()> {
        if descriptors.len() > 2 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut bytes = [0; LIMIT];
        let mut cursor = Cursor::new(bytes.as_mut_slice());
        serde_json::to_writer(
            &mut cursor,
            &Frame {
                schema: Revision,
                action,
            },
        )?;
        let len = usize::try_from(cursor.position()).map_err(io::Error::other)?;
        let mut storage = [0; rustix::cmsg_space!(ScmRights(2))];
        let mut control = SendAncillaryBuffer::new(&mut storage);
        if !descriptors.is_empty() && !control.push(SendAncillaryMessage::ScmRights(descriptors)) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let bytes = bytes.get(..len).ok_or(io::ErrorKind::InvalidData)?;
        let sent = net::sendmsg(
            &self.0,
            &[IoSlice::new(bytes)],
            &mut control,
            SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
        )?;
        if sent != len {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(())
    }

    pub(super) fn receive(&self) -> io::Result<Option<(Action, Vec<OwnedFd>)>> {
        let mut bytes = [0; LIMIT];
        let mut storage = [0; rustix::cmsg_space!(ScmRights(2))];
        let mut control = RecvAncillaryBuffer::new(&mut storage);
        let reply = match net::recvmsg(
            &self.0,
            &mut [IoSliceMut::new(&mut bytes)],
            &mut control,
            RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
        ) {
            Ok(reply) => reply,
            Err(rustix::io::Errno::AGAIN) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut descriptors = Vec::with_capacity(2);
        for message in control.drain() {
            let RecvAncillaryMessage::ScmRights(rights) = message else {
                return Err(io::ErrorKind::InvalidData.into());
            };
            for fd in rights {
                if descriptors.len() == 2 {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                descriptors.push(fd);
            }
        }
        if reply.bytes == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if !reply.flags.difference(RecvFlags::CMSG_CLOEXEC).is_empty() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let frame: Frame =
            serde_json::from_slice(bytes.get(..reply.bytes).ok_or(io::ErrorKind::InvalidData)?)?;
        let count = match frame.action {
            Action::Bind { .. } => 1,
            Action::Branch { .. } => 2,
            _ => 0,
        };
        if descriptors.len() != count {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(Some((frame.action, descriptors)))
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests;
