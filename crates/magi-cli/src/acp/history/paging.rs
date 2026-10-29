use anyhow::{Context, Result, ensure};
use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HistoryPage, SessionId, UiCommand};
use std::{io::Write, path::Path};

const COUNT: usize = 128;
const BYTES: usize = 8 * 1024 * 1024;

pub(super) struct Paging {
    session: SessionId,
    through: Cursor,
    next: Option<Cursor>,
    transcript: Option<String>,
}

struct Meter(usize);

impl Write for Meter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or(std::io::ErrorKind::FileTooLarge)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Paging {
    pub(super) fn new(
        session: SessionId,
        through: Cursor,
        after: Cursor,
        transcript: Option<String>,
    ) -> Result<Self> {
        ensure!(
            !session.as_str().is_empty()
                && session.as_str().len() <= super::NATIVE
                && after <= through,
            "invalid native history range"
        );
        ensure!(
            after == through || transcript.is_some(),
            "full history needs an expected transcript identity"
        );
        if let Some(transcript) = &transcript {
            super::external(transcript)?;
        }
        Ok(Self {
            session,
            through,
            next: Some(after),
            transcript,
        })
    }

    pub(super) async fn read(&mut self, socket: &Path, key: &str) -> Result<Option<HistoryPage>> {
        let Some(after) = self.next else {
            return Ok(None);
        };
        let page = tokio::time::timeout(crate::acp::transport::DEADLINE, async {
            let stream = magi_ipc::connect(socket).await?;
            let (read, write) = stream.into_split();
            FrameWriter::new(write)
                .write(&UiCommand::ReadHistory {
                    session: self.session.clone(),
                    through: self.through,
                    after,
                    key: Some(key.to_owned()),
                })
                .await?;
            FrameReader::new(read)
                .read::<std::result::Result<HistoryPage, String>>()
                .await?
                .map_err(|_| anyhow::anyhow!("native history read refused"))
        })
        .await
        .context("native history page deadline")??;
        self.accept(page).map(Some)
    }

    fn accept(&mut self, page: HistoryPage) -> Result<HistoryPage> {
        let after = self.next.context("native history is already complete")?;
        ensure!(
            page.session == self.session && page.through == self.through && page.after == after,
            "native history boundary changed"
        );
        let transcript = page
            .transcript
            .as_deref()
            .context("native transcript identity unavailable")?;
        super::external(transcript)?;
        ensure!(
            self.transcript
                .as_deref()
                .is_none_or(|expected| expected == transcript),
            "native transcript changed between pages"
        );
        ensure!(
            page.entries.len() <= COUNT,
            "native history page exceeds entry limit"
        );
        serde_json::to_writer(&mut Meter(BYTES), &page)
            .context("native history page exceeds byte limit")?;
        let mut last = after;
        for (cursor, _) in &page.entries {
            ensure!(
                *cursor > last && *cursor <= self.through,
                "native history cursor is repeated or outside the requested range"
            );
            last = *cursor;
        }
        match page.next {
            Some(next) => ensure!(
                !page.entries.is_empty() && next == last && next < self.through,
                "native history continuation does not advance exactly"
            ),
            None => ensure!(
                last == self.through,
                "native history ended before the requested boundary"
            ),
        }
        self.transcript = Some(transcript.to_owned());
        self.next = page.next;
        Ok(page)
    }
}

#[cfg(test)]
mod tests;
