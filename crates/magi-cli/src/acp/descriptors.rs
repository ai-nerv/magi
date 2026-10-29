use std::{
    io,
    os::fd::{AsFd, OwnedFd},
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd};

pub(super) struct Pollable(AsyncFd<OwnedFd>);

impl Pollable {
    pub(super) fn duplicate(fd: impl AsFd) -> io::Result<Self> {
        let owned = rustix::io::fcntl_dupfd_cloexec(fd, 3)?;
        let flags = rustix::fs::fcntl_getfl(&owned)?;
        rustix::fs::fcntl_setfl(&owned, flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(Self(AsyncFd::new(owned)?))
    }
}

impl AsyncRead for Pollable {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut ready = ready!(self.0.poll_read_ready(cx))?;
            match ready.try_io(|inner| {
                rustix::io::read(inner.get_ref(), buffer.initialize_unfilled())
                    .map_err(io::Error::from)
            }) {
                Ok(Ok(length)) => {
                    buffer.advance(length);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(why)) if why.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(why)) => return Poll::Ready(Err(why)),
                Err(_) => {}
            }
        }
    }
}

impl AsyncWrite for Pollable {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            let mut ready = ready!(self.0.poll_write_ready(cx))?;
            match ready
                .try_io(|inner| rustix::io::write(inner.get_ref(), bytes).map_err(io::Error::from))
            {
                Ok(Err(why)) if why.kind() == io::ErrorKind::Interrupted => {}
                Ok(result) => return Poll::Ready(result),
                Err(_) => {}
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
