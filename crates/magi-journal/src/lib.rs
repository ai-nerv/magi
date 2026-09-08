//! The transcript magi holds, and does not own.
//!
//! **balthasar is the store. This is a window on it.** Every read a session makes — a UI
//! attaching, the context a turn is built from, the usage total, a rewind point — comes from the
//! transcript, and a socket round trip per read would be absurd. So the entries are held here,
//! in memory, for as long as the session runs; they are *written* to balthasar through
//! `magi_host::scribe` and to nowhere else.
//!
//! **There is no file, and that is the whole point.** There was one: a JSONL journal per session
//! under `~/.local/share/magi/sessions/`. It is gone, because two stores is one store and a copy
//! that goes stale — and a stale copy of a conversation is worse than no copy, since it resumes
//! into something that half-happened. What is here holds nothing when the process ends, which is
//! correct: nothing here was ever the record.
//!
//! **Nothing in this crate touches the filesystem**, and that is checked rather than asserted —
//! see `scripts/gate-one-store.sh`. A writer added back here would be a second store nobody
//! declared, and it would look like a performance fix.

mod record;
mod recovery;

pub use record::{JOURNAL_VERSION, Record};
pub use recovery::{Recovered, parse};

use magi_proto::{Cursor, Entry, SessionId};

/// Anything that can go wrong holding a transcript.
///
/// One variant, and it is unreachable from this crate: nothing here can fail. It survives
/// because [`Journal::append`] and [`Journal::amend_at`] are called from a hundred places that
/// handle a `Result`, and because the *next* thing that can fail — balthasar refusing a write —
/// belongs in this shape rather than in a second one bolted beside it.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// The store refused, or could not be reached.
    #[error("the transcript could not be recorded: {0}")]
    Refused(String),
}

/// The transcript of one session, as this process holds it.
#[derive(Debug)]
pub struct Journal {
    session: SessionId,
    entries: Vec<Entry>,
    next: Cursor,
}

impl Journal {
    /// Hold a session's transcript, as balthasar replayed it.
    ///
    /// Entries arrive in cursor order and the next cursor follows the last of them, so a resumed
    /// session carries on numbering where it left off rather than overwriting its own history.
    #[must_use]
    pub fn recorded(session: SessionId, entries: Vec<Entry>) -> Self {
        let next = Cursor(entries.len() as u64).next();
        Self {
            session,
            entries,
            next,
        }
    }

    /// The session this transcript belongs to.
    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// The transcript, in order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Replace the last entry, for a message that is still arriving.
    ///
    /// Distinct from [`Journal::amend`] because that one is what gets streamed onward to the
    /// store, and streaming a growing answer per token would send the message once per token,
    /// each copy longer than the last. This keeps what is on screen current and says nothing to
    /// anybody.
    pub fn revise(&mut self, entry: Entry) {
        if let Some(last) = self.entries.last_mut() {
            *last = entry;
        }
    }

    /// Where the entry a cursor names sits, if it names one.
    ///
    /// Cursors count from one, so the first entry is at cursor 1. `None` for the zero cursor,
    /// which names the state before anything was written.
    fn at(cursor: Cursor) -> Option<usize> {
        usize::try_from(cursor.0).ok()?.checked_sub(1)
    }

    /// The position of the last entry.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor(self.next.0.saturating_sub(1))
    }

    /// Append an entry and return the position it took.
    ///
    /// # Errors
    /// Never, today. See [`JournalError`].
    pub fn append(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        let cursor = self.next;
        self.entries.push(entry);
        self.next = cursor.next();
        Ok(cursor)
    }

    /// Replace the last entry, for a message that was still streaming when it settled.
    ///
    /// # Errors
    /// Never, today. See [`JournalError`].
    pub fn amend(&mut self, entry: Entry) -> Result<Cursor, JournalError> {
        if self.entries.is_empty() {
            return self.append(entry);
        }
        let cursor = self.cursor();
        self.amend_at(cursor, entry)?;
        Ok(cursor)
    }

    /// Replace the entry at `cursor`, wherever it is.
    ///
    /// [`Journal::amend`] replaces the *last* entry, which is right for a message that is still
    /// streaming and wrong for anything else. A round of three tool calls commits three entries
    /// and then answers them one at a time: with only the last-entry form, the first two results
    /// landed on the third entry and were then overwritten by it, so two calls kept `result:
    /// null` for the rest of the session. What the model saw was two calls it had made and never
    /// got an answer to.
    ///
    /// # Errors
    /// Never, today. A cursor naming no entry is ignored rather than refused: it can only come
    /// from a caller holding a cursor from another session, and there is nothing to amend.
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
