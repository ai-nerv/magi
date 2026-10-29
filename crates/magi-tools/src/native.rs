use std::{
    io::{self, Write},
    ops::{Deref, DerefMut},
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Command, Stdio},
};

pub enum Input {
    Pipe(std::process::ChildStdin),
    Socket(UnixStream),
}

impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Pipe(input) => input.write(bytes),
            Self::Socket(input) => input.write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Pipe(input) => input.flush(),
            Self::Socket(input) => input.flush(),
        }
    }
}

/// A coordinator-prepared private bootstrap and its producer lifetime.
pub struct Handoff {
    packet: OwnedFd,
    input: UnixStream,
    lifetime: Box<dyn Send + Sync>,
}

impl Handoff {
    pub fn new(packet: OwnedFd, input: UnixStream, lifetime: impl Send + Sync + 'static) -> Self {
        Self {
            packet,
            input,
            lifetime: Box::new(lifetime),
        }
    }
}

/// Own the native process and retire its optional producer only after reaping.
pub struct Child {
    child: std::process::Child,
    pub input: Option<Input>,
    lifetime: Option<Box<dyn Send + Sync>>,
}

impl From<std::process::Child> for Child {
    fn from(mut child: std::process::Child) -> Self {
        let input = child.stdin.take().map(Input::Pipe);
        Self {
            child,
            input,
            lifetime: None,
        }
    }
}

impl Deref for Child {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.lifetime.take();
    }
}

pub fn spawn(mut command: Command, handoff: Option<Handoff>) -> io::Result<Child> {
    if let Some(Handoff {
        packet,
        input,
        lifetime,
    }) = handoff
    {
        command.arg("--event-bootstrap").stdin(Stdio::from(packet));
        let child = command.spawn()?;
        Ok(Child {
            child,
            input: Some(Input::Socket(input)),
            lifetime: Some(lifetime),
        })
    } else {
        command.spawn().map(Child::from)
    }
}

#[cfg(test)]
mod tests;
