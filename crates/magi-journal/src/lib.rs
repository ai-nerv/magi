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
    cursors: Vec<Cursor>,
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
            cursors: (1..next.0).map(Cursor).collect(),
            entries,
            next,
        }
    }

    /// Restore entries with their original strictly increasing, nonzero cursors.
    pub fn restore(session: SessionId, rows: Vec<(Cursor, Entry)>) -> Result<Self, JournalError> {
        let mut previous = 0;
        for (cursor, _) in &rows {
            if cursor.0 <= previous || cursor.0 == u64::MAX {
                return Err(JournalError::Refused(
                    "invalid transcript cursor ordering".into(),
                ));
            }
            previous = cursor.0;
        }
        let (cursors, entries) = rows.into_iter().unzip();
        Ok(Self {
            session,
            cursors,
            entries,
            next: Cursor(previous + 1),
        })
    }

    #[must_use]
    pub fn cursor_at(&self, index: usize) -> Option<Cursor> {
        self.cursors.get(index).copied()
    }

    #[must_use]
    pub fn position(&self, cursor: Cursor) -> Option<usize> {
        self.cursors.binary_search(&cursor).ok()
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

    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor(self.next.0.saturating_sub(1))
    }

    /// Append an entry at the next cursor, refusing an exhausted cursor range.
    pub fn append(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        if self.next.0 == u64::MAX {
            return Err(JournalError::Refused("transcript cursor exhausted".into()));
        }
        let cursor = self.next;
        self.entries.push(entry);
        self.cursors.push(cursor);
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
        let Some(at) = self.position(cursor) else {
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
