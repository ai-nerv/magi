use super::Session;
use magi_proto::Entry;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct Receipt {
    pub who: String,
    pub revision: String,
    pub loaded: bool,
}

#[derive(Default)]
pub(super) struct Reports {
    pending: BTreeMap<String, Receipt>,
    handled: BTreeMap<String, String>,
}

impl Session {
    pub(crate) fn track_report(&mut self, entry: &Entry) -> Result<Option<bool>, String> {
        let Entry::From {
            who, sort, text, ..
        } = entry
        else {
            return Ok(None);
        };
        if sort != "report" {
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|why| format!("invalid report notification: {why}"))?;
        let revision = value["revision"]
            .as_str()
            .filter(|revision| {
                revision.len() == 64 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or("report notification needs a SHA-256 revision")?;
        let reports = &mut self.admission.reports;
        if reports
            .handled
            .get(who)
            .is_some_and(|done| done == revision)
            || reports
                .pending
                .get(who)
                .is_some_and(|pending| pending.revision == revision)
        {
            return Ok(Some(false));
        }
        if reports.pending.len() >= 256 && !reports.pending.contains_key(who) {
            return Err("pending report queue is full".into());
        }
        reports.pending.insert(
            who.clone(),
            Receipt {
                who: who.clone(),
                revision: revision.to_owned(),
                loaded: false,
            },
        );
        Ok(Some(true))
    }

    pub(crate) fn pending_reports(&self) -> Vec<Receipt> {
        self.admission.reports.pending.values().cloned().collect()
    }

    pub(crate) fn report_loaded(&mut self, receipt: &Receipt) {
        if let Some(pending) = self.admission.reports.pending.get_mut(&receipt.who)
            && pending.revision == receipt.revision
        {
            pending.loaded = true;
        }
    }

    pub(crate) fn current_report(&mut self, receipt: &mut Receipt, revision: &str) -> bool {
        let reports = &mut self.admission.reports;
        if reports
            .handled
            .get(&receipt.who)
            .is_some_and(|done| done == revision)
        {
            reports.pending.remove(&receipt.who);
            return false;
        }
        if let Some(pending) = reports.pending.get_mut(&receipt.who)
            && pending.revision == receipt.revision
        {
            pending.revision = revision.to_owned();
            receipt.revision = revision.to_owned();
        }
        true
    }

    pub(crate) fn acknowledge_reports(&mut self, receipts: &[Receipt]) {
        let reports = &mut self.admission.reports;
        for receipt in receipts {
            if reports
                .pending
                .get(&receipt.who)
                .is_some_and(|pending| pending.loaded && pending.revision == receipt.revision)
            {
                reports.pending.remove(&receipt.who);
                reports
                    .handled
                    .insert(receipt.who.clone(), receipt.revision.clone());
            }
        }
        while reports.handled.len() > 1024 {
            reports.handled.pop_first();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::SessionId;

    fn notice(revision: &str) -> Entry {
        Entry::From {
            who: "demo/main/child".into(),
            kin: "child".into(),
            sort: "report".into(),
            text: serde_json::json!({"revision": revision}).to_string(),
        }
    }

    #[test]
    fn duplicates_are_coalesced_and_only_loaded_reports_are_acknowledged() {
        let mut session = Session::recorded(SessionId::new("r"), Vec::new());
        let event = notice(&"a".repeat(64));
        assert_eq!(
            session
                .track_report(&event)
                .expect("fixture operation succeeds"),
            Some(true)
        );
        assert_eq!(
            session
                .track_report(&event)
                .expect("fixture operation succeeds"),
            Some(false)
        );
        let receipts = session.pending_reports();
        session.acknowledge_reports(&receipts);
        assert_eq!(session.pending_reports().len(), 1);
        session.report_loaded(&receipts[0]);
        session.acknowledge_reports(&receipts);
        assert!(session.pending_reports().is_empty());
        assert_eq!(
            session
                .track_report(&event)
                .expect("fixture operation succeeds"),
            Some(false)
        );
    }

    #[test]
    fn acknowledging_an_old_revision_does_not_discard_a_new_report() {
        let mut session = Session::recorded(SessionId::new("r"), Vec::new());
        session
            .track_report(&notice(&"a".repeat(64)))
            .expect("fixture operation succeeds");
        let receipts = session.pending_reports();
        session.report_loaded(&receipts[0]);
        session
            .track_report(&notice(&"b".repeat(64)))
            .expect("fixture operation succeeds");
        session.acknowledge_reports(&receipts);
        assert_eq!(session.pending_reports()[0].revision, "b".repeat(64));
    }
}
