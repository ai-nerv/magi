use super::Session;
use magi_proto::{Cursor, HistoryPage, SessionId};
use std::io::Write;

const COUNT: usize = 128;
const BYTES: usize = 8 * 1024 * 1024;

struct Size {
    used: usize,
    limit: usize,
}

impl Write for Size {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.used) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.used += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Session {
    pub(crate) fn history_page(
        &self,
        session: &SessionId,
        through: Cursor,
        after: Cursor,
        transcript: Option<String>,
    ) -> Result<HistoryPage, String> {
        if self.busy() || self.id() != session || through != self.cursor() || after > through {
            return Err("history boundary is unavailable or changed".into());
        }
        if session.as_str().len() > 512 || transcript.as_ref().is_some_and(|id| id.len() > 512) {
            return Err("history identity exceeds its limit".into());
        }
        let start = if after == Cursor::ZERO {
            0
        } else {
            self.journal
                .position(after)
                .ok_or("unknown history cursor")?
                + 1
        };
        let mut page = HistoryPage {
            session: session.clone(),
            transcript,
            through,
            after,
            next: None,
            entries: Vec::new(),
        };
        let mut size = Size {
            used: 4096,
            limit: BYTES,
        };
        for (offset, entry) in self
            .entries()
            .get(start..)
            .ok_or("invalid history position")?
            .iter()
            .enumerate()
        {
            let cursor = self
                .cursor_at(start + offset)
                .ok_or("missing history cursor")?;
            if page.entries.len() == COUNT
                || serde_json::to_writer(&mut size, &(cursor, entry)).is_err()
            {
                page.next = Some(
                    page.entries
                        .last()
                        .ok_or("history entry exceeds page byte limit")?
                        .0,
                );
                break;
            }
            page.entries.push((cursor, entry.clone()));
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests;
