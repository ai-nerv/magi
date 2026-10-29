//! Complete bounded records from paged memory replies.

use super::{Scribe, ownership::Writer, rebuild, rows};
use magi_ipc::family::{DURABLE, Fault, MAX_FRAME_BYTES};
use magi_proto::{Cursor, Entry};
use serde_json::{Value, json};

const FRAGMENTS: usize = 256;

fn invalid() -> Fault {
    Fault::Malformed("invalid bounded raw history fragments".into())
}

struct Record {
    cursor: u64,
    bytes: usize,
    raw: String,
}

impl Record {
    fn new(row: &Value) -> Result<Self, Fault> {
        let cursor = row
            .get("cursor")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        let bytes = row
            .get("raw_bytes")
            .and_then(Value::as_u64)
            .ok_or_else(invalid)?;
        if cursor == 0 || cursor == u64::MAX || bytes == 0 || bytes > MAX_FRAME_BYTES as u64 {
            return Err(invalid());
        }
        Ok(Self {
            cursor,
            bytes: bytes as usize,
            raw: String::new(),
        })
    }

    fn append(&mut self, row: &Value) -> Result<bool, Fault> {
        let part = row.get("raw").and_then(Value::as_str).ok_or_else(invalid)?;
        let end = self.raw.len().checked_add(part.len()).ok_or_else(invalid)?;
        if part.is_empty()
            || end > self.bytes
            || row.get("cursor").and_then(Value::as_u64) != Some(self.cursor)
            || row.get("offset").and_then(Value::as_u64) != Some(self.raw.len() as u64)
            || row.get("raw_bytes").and_then(Value::as_u64) != Some(self.bytes as u64)
            || (end < self.bytes && row.get("next").and_then(Value::as_u64) != Some(end as u64))
            || (end == self.bytes && row.get("next").is_some())
        {
            return Err(invalid());
        }
        if self.raw.is_empty() {
            self.raw
                .try_reserve_exact(self.bytes)
                .map_err(|_| invalid())?;
        }
        self.raw.push_str(part);
        Ok(end == self.bytes)
    }
}

impl Scribe {
    pub(super) async fn replay_record(
        &mut self,
        id: &str,
        row: &Value,
        complete: bool,
    ) -> Result<(Cursor, Entry), Fault> {
        if row.get("truncated").and_then(Value::as_bool) != Some(true) {
            return rebuild(row);
        }
        let legacy = row.get("raw_bytes").is_none()
            || row
                .get("raw_bytes")
                .and_then(Value::as_u64)
                .is_some_and(|n| n > MAX_FRAME_BYTES as u64);
        let owned =
            matches!(&self.writer, Writer::Owned(key) if key == id && key == &self.transcript);
        if !complete && (legacy || !owned) {
            return rebuild(row);
        }
        if !owned {
            return Err(Fault::Failed(
                "complete raw history requires a writer reservation".into(),
            ));
        }
        self.check_writer()?;
        let record = Record::new(row)?;
        tokio::time::timeout(DURABLE, self.raw_record(id, record))
            .await
            .map_err(|_| {
                Fault::Unavailable("raw history exceeded its total read deadline".into())
            })?
    }

    async fn raw_record(&mut self, id: &str, mut record: Record) -> Result<(Cursor, Entry), Fault> {
        for _ in 0..FRAGMENTS {
            let values = self.link().await?.call_within(
                "replay",
                vec![json!(id), json!({"from":record.cursor,"raw":record.raw.len(),"bytes":super::REPLAY_PAGE})],
                DURABLE,
            ).await?;
            let mut fragments = values.iter().flat_map(rows);
            let row = fragments.next().ok_or_else(invalid)?;
            if fragments.next().is_some() {
                return Err(invalid());
            }
            if record.append(row)? {
                return rebuild(&json!({"cursor":record.cursor,"raw":record.raw}));
            }
        }
        Err(Fault::Malformed(
            "raw history exceeded its fragment count limit".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_metadata_is_bounded_before_fetch_or_allocation() {
        for bytes in [0, MAX_FRAME_BYTES as u64 + 1, u64::MAX] {
            assert!(Record::new(&json!({"cursor":1,"raw_bytes":bytes})).is_err());
        }
        for cursor in [0, u64::MAX] {
            assert!(Record::new(&json!({"cursor":cursor,"raw_bytes":1})).is_err());
        }
        let exact =
            Record::new(&json!({"cursor":1,"raw_bytes":MAX_FRAME_BYTES})).expect("exact bound");
        assert_eq!(exact.bytes, MAX_FRAME_BYTES);
        assert_eq!(exact.raw.capacity(), 0);
    }

    #[test]
    fn fragment_offsets_count_bytes_and_invalid_parts_never_advance() {
        let mut record = Record::new(&json!({"cursor":1,"raw_bytes":3})).expect("record");
        assert!(
            !record
                .append(&json!({"cursor":1,"raw_bytes":3,"offset":0,"raw":"é","next":2}))
                .expect("first")
        );
        let capacity = record.raw.capacity();
        assert!(capacity >= record.bytes);
        assert!(
            record
                .append(&json!({"cursor":1,"raw_bytes":3,"offset":2,"raw":"!"}))
                .expect("last")
        );
        assert_eq!(record.raw, "é!");
        assert_eq!(record.raw.capacity(), capacity);
        for (field, value) in [
            ("cursor", json!(2)),
            ("offset", json!(1)),
            ("raw_bytes", json!(4)),
            ("raw", json!("")),
            ("raw", json!("four")),
            ("next", Value::Null),
            ("next", json!(0)),
            ("next", json!(3)),
        ] {
            let mut record = Record::new(&json!({"cursor":1,"raw_bytes":3})).expect("record");
            let mut part = json!({"cursor":1,"raw_bytes":3,"offset":0,"raw":"é","next":2});
            part[field] = value;
            assert!(record.append(&part).is_err(), "{field}");
            assert_eq!(record.raw.len(), 0);
            assert_eq!(record.raw.capacity(), 0);
        }
        assert!(
            record
                .append(&json!({"cursor":1,"raw_bytes":3,"offset":3,"raw":"!"}))
                .is_err()
        );
        assert_eq!(record.raw, "é!");
    }
}
