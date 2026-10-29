use super::super::MAX_EVENT_BYTES;
use rustix::{
    fd::OwnedFd,
    net::{self, RecvAncillaryBuffer, RecvFlags, SendFlags, sockopt},
};
use std::io::IoSliceMut;

const REQUESTED_BUFFER: usize = 16 * 1024;
const MAX_BUFFER: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Loss {
    pub full: u64,
    pub unavailable: u64,
    pub invalid: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Queued,
    Full,
    Unavailable,
    Invalid,
}

pub struct Sender {
    socket: OwnedFd,
    loss: Loss,
}

impl Sender {
    pub(super) fn into_socket(self) -> OwnedFd {
        self.socket
    }

    pub(super) fn adopt(socket: OwnedFd) -> std::io::Result<Self> {
        use rustix::{
            fs::{OFlags, fcntl_getfl},
            io::{FdFlags, fcntl_getfd},
        };
        if !fcntl_getfd(&socket)?.contains(FdFlags::CLOEXEC)
            || !fcntl_getfl(&socket)?.contains(OFlags::NONBLOCK)
            || sockopt::get_socket_recv_buffer_size(&socket)? > MAX_BUFFER
            || sockopt::get_socket_send_buffer_size(&socket)? > MAX_BUFFER
        {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        Ok(Self {
            socket,
            loss: Loss::default(),
        })
    }

    pub fn send(&mut self, bytes: &[u8]) -> Sent {
        let (counter, result) = if bytes.is_empty() || bytes.len() > MAX_EVENT_BYTES {
            (&mut self.loss.invalid, Sent::Invalid)
        } else {
            match net::send(
                &self.socket,
                bytes,
                SendFlags::DONTWAIT | SendFlags::NOSIGNAL,
            ) {
                Ok(n) if n == bytes.len() => return Sent::Queued,
                Err(rustix::io::Errno::AGAIN) => (&mut self.loss.full, Sent::Full),
                _ => (&mut self.loss.unavailable, Sent::Unavailable),
            }
        };
        *counter = counter.saturating_add(1);
        result
    }

    pub fn loss(&self) -> Loss {
        self.loss
    }
}

pub(super) struct Channel {
    socket: OwnedFd,
}

impl Drop for Channel {
    fn drop(&mut self) {
        let _ = net::shutdown(&self.socket, net::Shutdown::ReadWrite);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Received {
    Frame(usize),
    Empty,
    Invalid,
    Unavailable,
}

impl Channel {
    pub(super) fn adopt(socket: OwnedFd) -> std::io::Result<Self> {
        private_socket(&socket, net::SocketType::DGRAM)?;
        let sender = Sender::adopt(socket)?;
        Ok(Self {
            socket: sender.into_socket(),
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        matches!(
            net::recv(
                &self.socket,
                &mut [0; 1],
                RecvFlags::DONTWAIT | RecvFlags::PEEK
            ),
            Err(rustix::io::Errno::AGAIN)
        )
    }

    pub(super) fn pair() -> std::io::Result<(Self, Sender)> {
        let (socket, sender) = pair()?;
        Ok((Self { socket }, sender))
    }

    pub(super) fn receive(&self, frame: &mut [u8; MAX_EVENT_BYTES]) -> Received {
        let mut ancillary = RecvAncillaryBuffer::default();
        match net::recvmsg(
            &self.socket,
            &mut [IoSliceMut::new(frame)],
            &mut ancillary,
            RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
        ) {
            Ok(reply)
                if reply.bytes == 0
                    || reply.bytes > frame.len()
                    || !reply.flags.difference(RecvFlags::CMSG_CLOEXEC).is_empty() =>
            {
                Received::Invalid
            }
            Ok(reply) => Received::Frame(reply.bytes),
            Err(rustix::io::Errno::AGAIN) => Received::Empty,
            Err(_) => Received::Unavailable,
        }
    }
}

pub(super) fn pair() -> std::io::Result<(OwnedFd, Sender)> {
    let (read, write) = net::socketpair(
        net::AddressFamily::UNIX,
        net::SocketType::DGRAM,
        net::SocketFlags::CLOEXEC | net::SocketFlags::NONBLOCK,
        None,
    )?;
    configure(&read)?;
    configure(&write)?;
    Ok((
        read,
        Sender {
            socket: write,
            loss: Loss::default(),
        },
    ))
}

pub(super) fn private_socket(
    socket: impl rustix::fd::AsFd,
    kind: net::SocketType,
) -> std::io::Result<()> {
    if sockopt::get_socket_type(&socket)? != kind
        || sockopt::get_socket_domain(&socket)? != net::AddressFamily::UNIX
        || sockopt::get_socket_peercred(&socket)?.uid != rustix::process::geteuid()
    {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let socket = std::os::unix::net::UnixDatagram::from(socket.as_fd().try_clone_to_owned()?);
    if !socket.local_addr()?.is_unnamed() || !socket.peer_addr()?.is_unnamed() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(())
}

pub(super) fn configure(socket: impl rustix::fd::AsFd) -> std::io::Result<()> {
    sockopt::set_socket_recv_buffer_size(&socket, REQUESTED_BUFFER)?;
    sockopt::set_socket_send_buffer_size(&socket, REQUESTED_BUFFER)?;
    if sockopt::get_socket_recv_buffer_size(&socket)? > MAX_BUFFER
        || sockopt::get_socket_send_buffer_size(&socket)? > MAX_BUFFER
    {
        return Err(std::io::ErrorKind::Unsupported.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
