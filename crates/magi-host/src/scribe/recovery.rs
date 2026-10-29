//! Fresh writer acquisition and bounded reconciliation after transport loss.

use super::{Scribe, ownership::Writer, rows};
use magi_ipc::family::{DURABLE, Family, Fault};
use magi_proto::{Cursor, Entry, SessionId};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

pub(super) type Digest = [u8; 32];

pub(super) fn digest(raw: &Value) -> Digest {
    Sha256::digest(raw.to_string().as_bytes()).into()
}

pub(super) fn fingerprint(cursor: Cursor, entry: &Entry) -> Result<Digest, Fault> {
    let raw = serde_json::to_value(magi_journal::Record::Entry {
        cursor,
        entry: entry.clone(),
    })
    .map_err(|why| Fault::Failed(format!("cannot fingerprint history: {why}")))?;
    Ok(digest(&raw))
}

impl Scribe {
    pub(super) async fn recover_writer(&mut self) -> Result<(), Fault> {
        let Writer::Owned(key) = &self.writer else {
            return Ok(());
        };
        if key != &self.transcript {
            return self.check_writer();
        }
        if self.family.as_ref().is_some_and(Family::has_connection) {
            return Ok(());
        }
        let now = std::time::Instant::now();
        if now < self.retry.0 {
            return Err(Fault::Unavailable("writer recovery is backing off".into()));
        }
        self.retry = (now + self.retry.1, (self.retry.1 * 2).min(super::BACKOFF.1));
        let recovered = self.reacquire().await.map_err(|why| {
            Fault::Failed(format!(
                "writer recovery refused; pending entries retained: {why}"
            ))
        })?;
        let (mut next, uncertain) = recovered;
        self.family = next.family.take();
        self.writer = next.writer;
        if let Some((cursor, digest)) = uncertain {
            self.sent.insert(cursor, digest);
        }
        self.uncertain = None;
        self.retry = (now, super::BACKOFF.0);
        Ok(())
    }

    async fn reacquire(&self) -> Result<(Self, Option<(u64, Digest)>), Fault> {
        let mut next = Self::unconnected(self.at.clone(), &SessionId::new(&self.session))
            .recording_as(self.transcript.clone());
        next.reserve_writer(true).await?;
        let values = next
            .link()
            .await?
            .call_within("resume", vec![json!(self.transcript)], DURABLE)
            .await?;
        let [owner] = values.as_slice() else {
            return Err(Fault::Failed("recovery requires one run identity".into()));
        };
        let run = owner.get("run").and_then(Value::as_str);
        let unbound = self.sent.is_empty() && run == Some(self.transcript.as_str());
        if run != Some(self.session.as_str()) && !unbound {
            return Err(Fault::Failed("the durable run identity changed".into()));
        }
        let uncertain = self.reconcile(&mut next).await?;
        if run != Some(self.session.as_str()) && uncertain.is_some() {
            return Err(Fault::Failed("the durable run identity changed".into()));
        }
        next.check_writer()?;
        Ok((next, uncertain))
    }

    async fn reconcile(&self, next: &mut Self) -> Result<Option<(u64, Digest)>, Fault> {
        let mut expected = self.sent.iter().peekable();
        let mut accepted = None;
        let mut from = 0;
        loop {
            let values = next
                .link()
                .await?
                .call_within(
                    "replay",
                    vec![
                        json!(self.transcript),
                        json!({"from":from,"bytes":super::REPLAY_PAGE}),
                    ],
                    DURABLE,
                )
                .await?;
            let page: Vec<_> = values.iter().flat_map(rows).collect();
            if page.is_empty() {
                if expected.next().is_some() {
                    return Err(Fault::Failed("acknowledged history is missing".into()));
                }
                return Ok(accepted);
            }
            for row in page {
                let (cursor, entry) = next.replay_record(&self.transcript, row, true).await?;
                if cursor.0 == 0 || cursor.0 < from || cursor.0 == u64::MAX {
                    return Err(Fault::Failed("recovery history is out of order".into()));
                }
                from = cursor.0 + 1;
                let actual = fingerprint(cursor, &entry)?;
                let pending = self.uncertain.filter(|(at, _)| *at == cursor.0);
                match expected.peek().copied() {
                    Some((&at, previous)) if at == cursor.0 => {
                        if actual != *previous && pending != Some((at, actual)) {
                            return Err(Fault::Failed("acknowledged history changed".into()));
                        }
                        expected.next();
                    }
                    Some((&at, _)) if at < cursor.0 => {
                        return Err(Fault::Failed("acknowledged history is missing".into()));
                    }
                    _ if pending != Some((cursor.0, actual)) => {
                        return Err(Fault::Failed("unrecognized durable history".into()));
                    }
                    _ => {}
                }
                if pending.is_some() {
                    accepted = Some((cursor.0, actual));
                }
            }
        }
    }
}
