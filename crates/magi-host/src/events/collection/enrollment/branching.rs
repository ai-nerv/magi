use super::*;
use crate::events::collection::branch::Branch;
use rustix::fd::AsFd;

impl Host {
    pub(super) fn enroll_branch(
        &mut self,
        producer: Id,
        parent: Lineage,
        descriptors: Vec<OwnedFd>,
        allowed: bool,
    ) -> io::Result<()> {
        if !parent.valid()
            || parent.run_id != self.lineage.run_id
            || parent.agent_id != self.lineage.agent_id
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let index = self
            .credits
            .iter()
            .position(|permit| permit.credit.identity == producer)
            .ok_or(io::ErrorKind::InvalidData)?;
        let permit = self.credits.swap_remove(index);
        let [events, control]: [OwnedFd; 2] = descriptors
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        let channel = transport::Channel::adopt(events)?;
        let socket = Socket::adopt(control)?;
        if !allowed || self.branch.is_some() {
            self.registrar.refuse();
            return Ok(());
        }
        let lineage = Lineage::root(
            parent.run_id.ok_or(io::ErrorKind::InvalidData)?,
            permit.agent,
        );
        let anchor = self
            .registrar
            .bind(permit.credit, Owner::Magi, lineage, channel)
            .map_err(|_| io::ErrorKind::InvalidData)?;
        self.branch = Some(Box::new(Self {
            socket,
            registrar: self.registrar.clone(),
            lineage,
            anchor: Some(anchor),
            credits: Vec::with_capacity(CREDITS),
            active: Vec::new(),
            branch: None,
            closed: false,
        }));
        Ok(())
    }
}

impl Client {
    /// Consume one cached root credit without waiting for nested admission.
    pub fn branch(&self, parent: Lineage) -> Result<Branch, RegisterError> {
        if self.is_closed() {
            return Err(RegisterError::Closed);
        }
        if !parent.valid()
            || parent.run_id != self.0.lineage.run_id
            || parent.agent_id != self.0.lineage.agent_id
        {
            return Err(RegisterError::Lineage);
        }
        let grant = self
            .0
            .credits
            .try_lock()
            .map_err(|_| RegisterError::Busy)?
            .pop()
            .ok_or(RegisterError::Busy)?;
        let prepare = (|| {
            let (events, sender) = transport::pair()?;
            let (control, peer) = wire::endpoints()?;
            let lineage = Lineage::root(
                parent.run_id.ok_or(io::ErrorKind::InvalidData)?,
                grant.agent,
            );
            let branch = Branch::new(
                Sender::new(sender, Owner::Magi, grant.producer).with_lineage(lineage),
                peer,
                parent,
            )
            .map_err(|_| io::ErrorKind::InvalidData)?;
            self.0.socket.send_descriptors(
                Action::Branch {
                    producer: grant.producer,
                    parent,
                },
                &[events.as_fd(), control.as_fd()],
            )?;
            Ok(branch)
        })();
        prepare
            .inspect_err(|_| self.0.close())
            .map_err(RegisterError::Transport)
    }
}

#[cfg(test)]
mod tests;
