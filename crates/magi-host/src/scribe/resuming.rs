use super::Scribe;
use magi_ipc::family::{Family, Fault};
use magi_journal::Journal;

impl Scribe {
    pub(crate) fn socket(&self) -> &std::path::Path {
        self.family.path()
    }

    pub(crate) async fn prepare_resume(&self, id: &str) -> Result<(Self, Journal), Fault> {
        let path = self.family.path().to_owned();
        let family = Family::dial(&path).await?;
        let mut next = Self::over(family, Some(path), &magi_proto::SessionId::new(id));
        let owner = next.run_of(id).await?;
        let rows = next.replay_at(id).await?;
        if rows.is_empty() {
            return Err(Fault::Refused(format!(
                "there is no nonempty transcript called {id:?}"
            )));
        }
        next.session = owner.as_str().to_owned();
        next.transcript = id.to_owned();
        next.sent = rows.iter().map(|(c, _)| c.0).collect();
        let journal =
            Journal::restore(owner, rows).map_err(|why| Fault::Malformed(why.to_string()))?;
        Ok((next, journal))
    }
}
