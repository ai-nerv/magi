//! Which agent the screen is pointed at, and what may be done to one that is not ours.
//!
//! Split out under THE RULE; the state it moves is next door.
//!
//! Two rules live here and nothing else does.
//!
//! **A swap forgets everything, the cursor included.** [`App::apply`](super::App::apply) folds a
//! cursor forward with `max` and never back, so a screen that had reached event 400 of its own
//! session would ask a peer for everything after *its* four-hundredth — and the session answers
//! that honestly by taking the first four hundred entries it does not have. The peer arrives with
//! no history and nothing anywhere is an error. It is the sharpest failure in the whole control.
//!
//! **Peeking at a peer is reading; driving it is not.** Every command the UI sends lands on
//! whoever the socket goes to, and the session on the other end cannot tell which of the two UIs
//! attached to it is the one that lives there. So the refusal is here, in front of the socket,
//! rather than on the wire.

use super::App;
use crate::melchior::Peer;
use magi_proto::{Cursor, Entry, UiCommand};

/// Where a step left the screen pointed.
///
/// The own case carries no path: this session's socket is the driver's, named from a key made
/// before any of this existed, and a copy kept here would be a second opinion about a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seat {
    /// This session's own socket, whatever the driver called it.
    Own,
    /// A peer's screen, at the path melchior published beside its name.
    Peer(std::path::PathBuf),
}

impl App {
    /// This session's own id: the third part of `project/role/id`.
    ///
    /// `None` for a session with no melchior, which is called after its project and has no
    /// siblings to be told apart from.
    fn my_id(&self) -> Option<&str> {
        self.named.split('/').nth(2)
    }

    /// Whether the screen could actually be moved to this peer.
    ///
    /// **A screen, and not us.** melchior's roster is everyone listening in the project, which
    /// includes this session — it dials its own socket like any other — and an agent that
    /// published no `.ui` note has a name and nowhere to look.
    fn movable(&self, them: &Peer) -> bool {
        them.ui.is_some() && Some(them.id.as_str()) != self.my_id()
    }

    /// How many agents there are to move between, this session included.
    ///
    /// What the footer draws its arrows from, so it counts what the arrows can actually reach
    /// rather than what melchior can name. A project full of peers that published no screen is a
    /// crew of one, and a crew of one draws no control.
    #[must_use]
    pub fn crew_size(&self) -> usize {
        1 + self
            .reachable
            .iter()
            .filter(|them| self.movable(them))
            .count()
    }

    /// The peers the screen can be moved to, in a fixed order.
    ///
    /// Sorted by id, because the ring must not reorder under somebody halfway round it: melchior
    /// republishes the whole roster whenever any part of it changes, and an agent that took a role
    /// would otherwise move everybody after it one place along.
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

    /// Point the screen at another agent, and forget everything the last one said.
    ///
    /// Everything: the transcript belongs to whoever sent it, and so does the trace, the
    /// scrollback, whatever list was open over it and every screen position recorded against
    /// lines that are about to be replaced. What is not obvious is the cursor — see the note at
    /// the top of this file, which is the bug this method exists for.
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
        // What arrived for *this* session while the screen was elsewhere is this session's, and
        // it has been counted against a transcript that is now somebody else's.
        self.waiting = 0;
    }

    /// Step one place along the crew, and say what to dial.
    ///
    /// `None` when there is nowhere to go, which is the ordinary case: a session that started
    /// nothing is alone in its project and both arrows are a no-op it is not even shown.
    pub fn step(&mut self, forward: bool) -> Option<Seat> {
        let crew = self.crew();
        if crew.is_empty() {
            return None;
        }
        // Position in the ring, with this session at nought. An attachment to somebody who has
        // since gone counts as nought rather than as a place in the list: the ring it was a part
        // of no longer has it, so the next arrow starts again from this session.
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

    /// What the footer calls whoever is on screen.
    ///
    /// A peer is `role/id` rather than the full `project/role/id`: the project is the one part
    /// that cannot differ — melchior's roster is a single project's — and the row it is drawn on
    /// loses whole columns to make room.
    #[must_use]
    pub fn viewing(&self) -> String {
        match &self.attached {
            None => self.named.clone(),
            Some(them) if them.role.is_empty() => them.id.clone(),
            Some(them) => format!("{}/{}", them.role, them.id),
        }
    }

    /// Say why a keystroke did nothing, once.
    ///
    /// Once, because the keys this refuses are keys people lean on. Ten identical lines above the
    /// prompt say no more than one and cost the whole screen.
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

/// Whether this command would change the session it lands on.
///
/// An allow-list of the two that would not, rather than a list of the ones that would: a command
/// added later is a command nobody remembered to gate, and the safe default for a screen that is
/// only reading is to send nothing. `Sized` is on the driving side and belongs there — it grants
/// rows out of *this* terminal, and a peer whose own UI is a different shape would lay a tool out
/// for a window nobody is looking at.
#[must_use]
pub fn drives(command: &UiCommand) -> bool {
    !matches!(command, UiCommand::Attach { .. } | UiCommand::Detach)
}

/// Whether refusing it is worth a sentence.
///
/// Only what a person pressed a key for. A width, a keystroke on somebody else's surface and a
/// pointer are the machinery answering the terminal, and a notice per mouse move would bury the
/// transcript it was printed over.
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
