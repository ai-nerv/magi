//! The transcript magi holds, and does not own.
//!
//! balthasar is the store; this is a window on it, held in memory for the life of the session and
//! written through `magi_host::scribe` and nowhere else. Nothing here touches the filesystem.

mod record;
mod recovery;

pub use record::{JOURNAL_VERSION, Record};
pub use recovery::{Recovered, parse};

use magi_proto::{Cursor, Entry, SessionId};

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("the transcript could not be recorded: {0}")]
    Refused(String),
}

#[derive(Debug)]
pub struct Journal {
    session: SessionId,
    entries: Vec<Entry>,
    next: Cursor,
}

impl Journal {
    /// Hold a session's transcript, as balthasar replayed it. Entries arrive in cursor order and
    /// the next cursor follows the last of them, so a resumed session carries on numbering.
    #[must_use]
    pub fn recorded(session: SessionId, entries: Vec<Entry>) -> Self {
        let next = Cursor(entries.len() as u64).next();
        Self {
            session,
            entries,
            next,
        }
    }

    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Replace the last entry, for a message that is still arriving. Unlike [`Journal::amend`]
    /// this is not streamed onward to the store, which would resend the message once per token.
    pub fn revise(&mut self, entry: Entry) {
        if let Some(last) = self.entries.last_mut() {
            *last = entry;
        }
    }

    /// Where the entry a cursor names sits. Cursors count from one; the zero cursor names none.
    fn at(cursor: Cursor) -> Option<usize> {
        usize::try_from(cursor.0).ok()?.checked_sub(1)
    }

    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor(self.next.0.saturating_sub(1))
    }

    /// Append an entry and return the position it took. Cannot fail today; see [`JournalError`].
    pub fn append(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        let cursor = self.next;
        self.entries.push(entry);
        self.next = cursor.next();
        Ok(cursor)
    }

    /// Replace the last entry, for a message that was still streaming when it settled.
    pub fn amend(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        if self.entries.is_empty() {
            return self.append(entry);
        }
        let cursor = self.cursor();
        self.amend_at(cursor, entry)?;
        Ok(cursor)
    }

    /// Replace the entry at `cursor`, wherever it is. [`Journal::amend`] replaces the last entry,
    /// which is wrong for a round of tool calls answered one at a time. A cursor naming no entry
    /// is ignored rather than refused.
    pub fn amend_at(&mut self, cursor: Cursor, entry: Entry) -> Result<(), JournalError> {
        let Some(at) = Self::at(cursor) else {
            return Ok(());
        };
        let Some(slot) = self.entries.get_mut(at) else {
            return Ok(());
        };
        *slot = entry;
        Ok(())
    }
}

#[cfg(test)]
#[path = "holding.rs"]
mod holding;
