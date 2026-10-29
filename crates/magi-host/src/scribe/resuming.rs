use super::Scribe;
use magi_ipc::family::{Family, Fault};
use magi_journal::Journal;

impl Scribe {
    pub(crate) fn transcript(&self) -> &str {
        &self.transcript
    }

    pub(crate) fn socket(&self) -> &std::path::Path {
        &self.at
    }

    /// Prepare another transcript on its own connection, reserving before any history read.
    pub async fn load_for(
        &self,
        id: &str,
        required: bool,
    ) -> Result<(Self, Vec<(magi_proto::Cursor, magi_proto::Entry)>), Fault> {
        let path = self.at.clone();
        let family = Family::dial(&path).await?;
        let mut next = Self::over(family, Some(path), &magi_proto::SessionId::new(id));
        next.reserve_writer(required).await?;
        let owner = next.run_of(id).await?;
        let rows = next.replay_at(id).await?;
        if rows.is_empty() {
            return Err(Fault::Refused(format!(
                "there is no nonempty transcript called {id:?}"
            )));
        }
        next.session = owner.as_str().to_owned();
        next.transcript = id.to_owned();
        next.sent = rows
            .iter()
            .map(|(cursor, entry)| {
                super::recovery::fingerprint(*cursor, entry).map(|digest| (cursor.0, digest))
            })
            .collect::<Result<_, _>>()?;
        Ok((next, rows))
    }

    pub(crate) async fn prepare_resume(
        &mut self,
        id: &str,
    ) -> Result<(Option<Self>, Journal), Fault> {
        if self.transcript == id {
            self.reserve_writer(false).await?;
            let owner = self.run_of(id).await?;
            let rows = self.replay_at(id).await?;
            if rows.is_empty() {
                return Err(Fault::Refused(format!(
                    "there is no nonempty transcript called {id:?}"
                )));
            }
            let journal =
                Journal::restore(owner, rows).map_err(|why| Fault::Malformed(why.to_string()))?;
            return Ok((None, journal));
        }
        let (next, rows) = self.load_for(id, false).await?;
        let journal = Journal::restore(magi_proto::SessionId::new(&next.session), rows)
            .map_err(|why| Fault::Malformed(why.to_string()))?;
        Ok((Some(next), journal))
    }

    pub(crate) fn accept_resume(&mut self, journal: &Journal) -> Result<(), Fault> {
        let sent = journal
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(n, entry)| journal.cursor_at(n).map(|cursor| (cursor, entry)))
            .map(|(cursor, entry)| {
                super::recovery::fingerprint(cursor, entry).map(|digest| (cursor.0, digest))
            })
            .collect::<Result<_, _>>()?;
        self.session = journal.session().as_str().to_owned();
        self.sent = sent;
        self.uncertain = None;
        Ok(())
    }
}
