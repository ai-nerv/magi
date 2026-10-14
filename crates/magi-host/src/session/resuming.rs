use super::Session;
use magi_journal::{Journal, JournalError};
use magi_proto::{AgentStatus, Cursor, Entry, SessionId};

#[cfg(test)]
mod tests;

/// The most entries kept waiting for a memory layer that is not answering.
pub(crate) const PENDING_MOST: usize = 2048;

/// The most text those entries may hold between them.
pub(crate) const PENDING_BYTES: usize = 64 * 1024 * 1024;

/// Roughly how much text an entry holds.
pub(super) fn weight(entry: &Entry) -> usize {
    match entry {
        Entry::User { text, aside, .. } => text.len() + aside.len(),
        Entry::Assistant { text, thinking, .. } => text.len() + thinking.len(),
        Entry::Tool { args, result, .. } => {
            args.len() + result.as_ref().map_or(0, |r| r.output.len())
        }
        Entry::Notice { text } | Entry::From { text, .. } => text.len(),
        Entry::Compaction { summary, .. } => summary.len(),
        Entry::Masked { shown, .. } => shown.len(),
        Entry::Branch { .. } => 0,
    }
}

impl Session {
    pub(crate) fn helpers(&self) -> crate::settling::Tasks {
        self.helpers.clone()
    }

    pub fn restored(id: SessionId, rows: Vec<(Cursor, Entry)>) -> Result<Self, JournalError> {
        let journal = Journal::restore(id.clone(), rows)?;
        let mut session = Self::recorded(id, Vec::new());
        session.journal = journal;
        Ok(session)
    }

    pub fn cursor_at(&self, index: usize) -> Option<Cursor> {
        self.journal.cursor_at(index)
    }

    pub fn position(&self, cursor: Cursor) -> Option<usize> {
        self.journal.position(cursor)
    }

    pub(crate) fn pending_batch(&self) -> Vec<(Cursor, Entry)> {
        self.pending
            .iter()
            .map(|(cursor, entry)| (Cursor(*cursor), entry.clone()))
            .collect()
    }

    pub(crate) fn acknowledge_pending(&mut self, cursor: Cursor, entry: &Entry) {
        if self.pending.get(&cursor.0) == Some(entry) {
            self.pending.remove(&cursor.0);
        }
    }

    /// Let go of the oldest entries waiting to be recorded once there are more than
    /// [`PENDING_MOST`] of them or they hold more than [`PENDING_BYTES`], and say which went.
    pub(crate) fn bound_pending(&mut self) -> Option<String> {
        let mut bytes: usize = self.pending.values().map(weight).sum();
        let mut gone: Option<(u64, u64, usize)> = None;
        while self.pending.len() > PENDING_MOST || bytes > PENDING_BYTES {
            let Some((cursor, entry)) = self.pending.pop_first() else {
                break;
            };
            bytes = bytes.saturating_sub(weight(&entry));
            let (first, _, count) = gone.unwrap_or((cursor, cursor, 0));
            gone = Some((first, cursor, count + 1));
        }
        let (first, last, count) = gone?;
        Some(format!(
            "The memory layer is still not answering, so {count} of the oldest entries waiting \
             to be recorded (cursors {first} to {last}) were let go to keep the queue bounded. \
             They stay on screen, and will not be in the store."
        ))
    }

    pub(crate) fn resume_prepared(&mut self, journal: Journal) {
        self.journal = journal;
        self.admission.reports = Default::default();
        self.cancel = Default::default();
        self.helpers = Default::default();
        self.hints.clear();
        self.laid = None;
        self.rested = None;
        self.deferred.clear();
        self.helpers_spent = Default::default();
        self.pending.clear();
        self.tallied.clear();
        self.counted.clear();
        self.spent.send_replace(Vec::new());
        for entry in self.entries().to_vec() {
            self.tally(&entry);
        }
        self.status = AgentStatus::Idle;
        self.phase.send_replace(AgentStatus::Idle);
        let _ = self.events.send(self.snapshot(self.cursor()));
    }
}
