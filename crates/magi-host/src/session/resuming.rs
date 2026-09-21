use super::Session;
use magi_journal::{Journal, JournalError};
use magi_proto::{AgentStatus, Cursor, Entry, SessionId};

#[cfg(test)]
mod tests;

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

    pub(crate) fn resume_prepared(&mut self, journal: Journal) {
        self.journal = journal;
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
