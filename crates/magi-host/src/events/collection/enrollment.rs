mod branching;
mod wire;

use super::{
    Lifetime, MAX_PRODUCERS, Owner, RegisterError, Registrar, Sender, transport, worker::Credit,
};
use crate::events::{Id, Lineage};
use rustix::fd::OwnedFd;
use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use wire::{Action, Socket};

const CREDITS: usize = 2;

struct Permit {
    credit: Credit,
    agent: Id,
}

#[derive(Clone, Copy)]
struct Grant {
    producer: Id,
    agent: Id,
}

/// Root-owned admission and lifetimes for one child host and its native suppliers.
pub struct Host {
    socket: Socket,
    registrar: Registrar,
    lineage: Lineage,
    anchor: Option<Lifetime>,
    credits: Vec<Permit>,
    active: Vec<(Id, Lifetime)>,
    branch: Option<Box<Host>>,
    closed: bool,
}

impl Host {
    pub fn pair(registrar: Registrar, lineage: Lineage) -> io::Result<(Self, Sender, OwnedFd)> {
        if !process_binding(lineage) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let opened = registrar
            .open_bound(Owner::Magi, lineage)
            .map_err(|_| io::Error::other("event enrollment unavailable"))?;
        let (socket, peer) = wire::pair()?;
        let mut host = Self {
            socket,
            registrar,
            lineage,
            anchor: Some(opened.lifetime),
            credits: Vec::with_capacity(CREDITS),
            active: Vec::new(),
            branch: None,
            closed: false,
        };
        for _ in 0..CREDITS {
            host.offer()?;
        }
        Ok((host, opened.sender, peer))
    }

    /// Process at most one request and offer at most one replacement credit.
    pub fn poll(&mut self) -> bool {
        self.poll_branches(false)
    }

    pub(super) fn poll_branches(&mut self, allow_branch: bool) -> bool {
        if self.closed {
            return false;
        }
        let result = self.socket.receive().and_then(|packet| match packet {
            Some((
                Action::Bind {
                    producer,
                    program,
                    lineage,
                },
                mut descriptors,
            )) => {
                let socket = descriptors.pop().ok_or(io::ErrorKind::InvalidData)?;
                self.bind(producer, program, lineage, socket)
            }
            Some((Action::Branch { producer, parent }, descriptors)) => {
                self.enroll_branch(producer, parent, descriptors, allow_branch)
            }
            Some((Action::Retire { producer }, _)) => {
                let index = self
                    .active
                    .iter()
                    .position(|(id, _)| *id == producer)
                    .ok_or(io::ErrorKind::InvalidData)?;
                self.active.swap_remove(index);
                Ok(())
            }
            None => Ok(()),
            _ => Err(io::ErrorKind::InvalidData.into()),
        });
        if let Err(error) = result {
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
            ) {
                self.registrar.refuse();
            }
            self.close();
        } else if self.offer().is_err() {
            self.close();
        }
        !self.closed
    }

    pub(super) fn take_branch(&mut self) -> Option<Self> {
        self.branch.take().map(|host| *host)
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.socket.close();
        self.credits.clear();
        self.active.clear();
        self.anchor.take();
    }

    fn offer(&mut self) -> io::Result<()> {
        if self.credits.len() >= CREDITS
            || self.active.len() + self.credits.len() >= MAX_PRODUCERS - 1
            || !self.registrar.available()
        {
            return Ok(());
        }
        let credit = self
            .registrar
            .reserve()
            .map_err(|_| io::Error::other("event enrollment unavailable"))?;
        let agent = Id::generate().map_err(io::Error::other)?;
        self.socket.send(
            Action::Offer {
                producer: credit.identity,
                agent,
            },
            None,
        )?;
        self.credits.push(Permit { credit, agent });
        Ok(())
    }

    fn bind(
        &mut self,
        producer: Id,
        program: Owner,
        lineage: Lineage,
        socket: OwnedFd,
    ) -> io::Result<()> {
        if program == Owner::Magi
            || !lineage.valid()
            || lineage.run_id != self.lineage.run_id
            || lineage.agent_id != self.lineage.agent_id
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let index = self
            .credits
            .iter()
            .position(|permit| permit.credit.identity == producer)
            .ok_or(io::ErrorKind::InvalidData)?;
        let channel = transport::Channel::adopt(socket)?;
        let lifetime = self
            .registrar
            .bind(
                self.credits.swap_remove(index).credit,
                program,
                lineage,
                channel,
            )
            .map_err(|_| io::ErrorKind::InvalidData)?;
        self.active.push((producer, lifetime));
        Ok(())
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.close();
    }
}

struct State {
    socket: Socket,
    lineage: Lineage,
    credits: Mutex<Vec<Grant>>,
    closed: AtomicBool,
}

impl State {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.socket.close();
    }

    fn send(&self, action: Action, descriptor: Option<&OwnedFd>) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.socket
            .send(action, descriptor)
            .inspect_err(|_| self.close())
    }
}

/// A child-local credit cache; launches never wait for a root response.
pub struct Client(Arc<State>);

pub struct Opened {
    pub sender: Sender,
    pub lifetime: RemoteLifetime,
}

pub struct RemoteLifetime {
    state: Arc<State>,
    producer: Id,
}

impl Drop for RemoteLifetime {
    fn drop(&mut self) {
        let _ = self.state.send(
            Action::Retire {
                producer: self.producer,
            },
            None,
        );
    }
}

impl Client {
    pub fn is_closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }

    pub fn adopt(socket: OwnedFd, lineage: Lineage) -> io::Result<Self> {
        if !process_binding(lineage) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let client = Self(Arc::new(State {
            socket: Socket::adopt(socket)?,
            lineage,
            credits: Mutex::new(Vec::with_capacity(CREDITS)),
            closed: AtomicBool::new(false),
        }));
        for _ in 0..CREDITS {
            client.poll()?;
        }
        Ok(client)
    }

    /// Receive at most one credit on the collection worker.
    pub fn poll(&self) -> io::Result<()> {
        let result = (|| match self.0.socket.receive()? {
            Some((Action::Offer { producer, agent }, _)) => {
                let mut credits = self
                    .0
                    .credits
                    .lock()
                    .map_err(|_| io::ErrorKind::BrokenPipe)?;
                if credits.len() >= CREDITS
                    || producer == agent
                    || agent == self.0.lineage.agent_id.ok_or(io::ErrorKind::InvalidData)?
                    || credits
                        .iter()
                        .any(|grant| grant.producer == producer || grant.agent == agent)
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                credits.push(Grant { producer, agent });
                Ok(())
            }
            None => Ok(()),
            _ => Err(io::ErrorKind::InvalidData.into()),
        })();
        result.inspect_err(|_| self.0.close())
    }

    pub fn open(&self, owner: Owner, lineage: Lineage) -> Result<Opened, RegisterError> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(RegisterError::Closed);
        }
        if owner == Owner::Magi
            || !lineage.valid()
            || lineage.run_id != self.0.lineage.run_id
            || lineage.agent_id != self.0.lineage.agent_id
        {
            return Err(RegisterError::Lineage);
        }
        let producer = self
            .0
            .credits
            .try_lock()
            .map_err(|_| RegisterError::Busy)?
            .pop()
            .ok_or(RegisterError::Busy)?
            .producer;
        let (socket, sender) = transport::pair().map_err(|error| {
            self.0.close();
            RegisterError::Transport(error)
        })?;
        self.0
            .send(
                Action::Bind {
                    producer,
                    program: owner,
                    lineage,
                },
                Some(&socket),
            )
            .map_err(RegisterError::Transport)?;
        Ok(Opened {
            sender: Sender::new(sender, owner, producer).with_lineage(lineage),
            lifetime: RemoteLifetime {
                state: self.0.clone(),
                producer,
            },
        })
    }

    pub fn close(&self) {
        self.0.close();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

fn process_binding(lineage: Lineage) -> bool {
    lineage.run_id.is_some()
        && lineage.agent_id.is_some()
        && lineage.turn_id.is_none()
        && lineage.ask_id.is_none()
}

#[cfg(test)]
mod tests;
