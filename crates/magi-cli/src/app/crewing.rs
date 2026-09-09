//! Which agent the screen is pointed at, and what may be done to one that is not ours. Driving a
//! peer is refused here, in front of the socket, rather than on the wire: the session on the other
//! end cannot tell which of the UIs attached to it is the one that lives there.

use super::App;
use crate::melchior::Peer;
use magi_proto::{Cursor, Entry, UiCommand};

/// Where a step left the screen pointed. The own seat carries no path: the driver names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seat {
    Own,
    Peer(std::path::PathBuf),
}

impl App {
    /// This session's own id: the third part of `project/role/id`. `None` without a melchior.
    fn my_id(&self) -> Option<&str> {
        self.named.split('/').nth(2)
    }

    /// Whether the screen could be moved to this peer: not us, and it published a `.ui` note.
    fn movable(&self, them: &Peer) -> bool {
        them.ui.is_some() && Some(them.id.as_str()) != self.my_id()
    }

    /// How many agents the arrows can reach, this session included.
    #[must_use]
    pub fn crew_size(&self) -> usize {
        1 + self
            .reachable
            .iter()
            .filter(|them| self.movable(them))
            .count()
    }

    /// The peers the screen can be moved to, sorted by id so the ring does not reorder under
    /// somebody halfway round it when melchior republishes the roster.
    fn crew(&self) -> Vec<Peer> {
        let mut found: Vec<Peer> = self
            .reachable
            .iter()
            .filter(|them| self.movable(them))
            .cloned()
            .collect();
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found
    }

    /// Point the screen at another agent, and forget everything the last one said. The cursor
    /// must go too: [`App::apply`](super::App::apply) folds it forward with `max` and never back,
    /// so a stale cursor makes the peer skip that many entries and arrive with no history.
    pub fn attach_to(&mut self, them: Option<Peer>) {
        self.attached = them;
        self.cursor = Cursor::ZERO;
        self.entries.clear();
        self.timeline = magi_tui::trace::Trace::new();
        self.scrollback = magi_tui::scrollback::Scrollback::new();
        self.overlay = None;
        self.pane = None;
        self.picking = None;
        self.surface = None;
        self.model = None;
        self.choices.clear();
        self.set_status(magi_proto::AgentStatus::Idle);
        self.selection = None;
        self.hovering = None;
        self.flipped.clear();
        self.owners.clear();
        self.blocks.clear();
        self.waiting = 0;
    }

    /// Step one place along the crew, and say what to dial. `None` when there is nowhere to go.
    pub fn step(&mut self, forward: bool) -> Option<Seat> {
        let crew = self.crew();
        if crew.is_empty() {
            return None;
        }
        // Position in the ring, with this session at nought. An attachment to somebody who has
        // since gone counts as nought, so the next arrow starts again from this session.
        let now = self.attached.as_ref().map_or(0, |here| {
            crew.iter()
                .position(|them| them.id == here.id)
                .map_or(0, |at| at + 1)
        });
        let round = crew.len() + 1;
        let next = if forward {
            (now + 1) % round
        } else {
            (now + round - 1) % round
        };
        let target = next.checked_sub(1).map(|at| crew[at].clone());
        let seat = match target.as_ref().and_then(|them| them.ui.clone()) {
            Some(at) => Seat::Peer(at),
            // Only reachable for the own seat: `movable` is what put the rest in the ring.
            None if target.is_none() => Seat::Own,
            None => return None,
        };
        self.attach_to(target);
        Some(seat)
    }

    /// What the footer calls whoever is on screen. A peer is `role/id`: melchior's roster is a
    /// single project's, so the project part cannot differ.
    #[must_use]
    pub fn viewing(&self) -> String {
        match &self.attached {
            None => self.named.clone(),
            Some(them) if them.role.is_empty() => them.id.clone(),
            Some(them) => format!("{}/{}", them.role, them.id),
        }
    }

    /// Say why a keystroke did nothing, once — these are keys people lean on.
    pub fn refuse_drive(&mut self) {
        if self.attached.is_none() {
            return;
        }
        let said = format!(
            "You are reading `{}`, not driving it. `alt+,` and `alt+.` move between agents — go back to `{}` before you type.",
            self.viewing(),
            self.named,
        );
        if matches!(self.entries.last(), Some(Entry::Notice { text }) if *text == said) {
            return;
        }
        self.show_notice(said);
    }
}

/// Whether this command would change the session it lands on. An allow-list of the two that would
/// not, so a command added later defaults to blocked.
#[must_use]
pub fn drives(command: &UiCommand) -> bool {
    !matches!(command, UiCommand::Attach { .. } | UiCommand::Detach)
}

/// Whether refusing it is worth a sentence: only what a person pressed a key for, since a notice
/// per mouse move would bury the transcript.
#[must_use]
pub fn spoken(command: &UiCommand) -> bool {
    !matches!(
        command,
        UiCommand::Sized { .. } | UiCommand::Keyed { .. } | UiCommand::Moused { .. }
    )
}

#[cfg(test)]
#[path = "crewing/tests.rs"]
mod tests;
