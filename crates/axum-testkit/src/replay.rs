//! Replay a recorded session over the wire.

use axum_ipc::{FrameReader, FrameWriter, IpcError, PeerCred};
use axum_proto::{
    AgentStatus, Cursor, Entry, HarnessEvent, MessageId, SessionId, ToolCallId, UiCommand,
};
use std::path::Path;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};

/// A recorded session: a list of events, one JSON object per line.
#[derive(Debug, Default, Clone)]
pub struct Recording {
    events: Vec<HarnessEvent>,
}

impl Recording {
    /// Parse a JSONL recording.
    ///
    /// Blank lines and `#` comments are skipped so a recording stays hand-editable, which is
    /// the entire point of keeping this format JSON rather than the CBOR used on the wire.
    pub fn parse(source: &str) -> Result<Self, serde_json::Error> {
        let mut events = Vec::new();
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            events.push(serde_json::from_str(line)?);
        }
        Ok(Self { events })
    }

    /// Read a JSONL recording from disk.
    pub async fn load(path: &Path) -> std::io::Result<Self> {
        let source = tokio::fs::read_to_string(path).await?;
        Self::parse(&source).map_err(std::io::Error::other)
    }

    /// The recorded events.
    #[must_use]
    pub fn events(&self) -> &[HarnessEvent] {
        &self.events
    }

    /// How many events were recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Fold the events at or before `cursor` into the transcript they produce.
    ///
    /// This is what makes cold attach real rather than a replay: a UI that reconnects mid-turn
    /// gets the finished history as entries and only the remainder as events, so an in-flight
    /// assistant message arrives already carrying the text it had streamed so far.
    #[must_use]
    pub fn snapshot_at(&self, cursor: Cursor) -> (Vec<Entry>, AgentStatus) {
        let mut entries: Vec<Entry> = Vec::new();
        let mut status = AgentStatus::Idle;

        for event in self.events.iter().filter(|e| e.cursor() <= cursor) {
            match event.clone() {
                HarnessEvent::UserMessage { id, text, .. } => {
                    entries.push(Entry::User { id, text });
                }
                HarnessEvent::AssistantStarted { id, .. } => entries.push(Entry::Assistant {
                    id,
                    text: String::new(),
                    thinking: String::new(),
                    stop_reason: None,
                    error: None,
                }),
                HarnessEvent::AssistantDelta {
                    id, text, thinking, ..
                } => {
                    if let Some(Entry::Assistant {
                        text: body,
                        thinking: reasoning,
                        ..
                    }) = find_assistant(&mut entries, &id)
                    {
                        body.push_str(&text);
                        reasoning.push_str(&thinking);
                    }
                }
                HarnessEvent::AssistantEnded {
                    id,
                    stop_reason,
                    error,
                    ..
                } => {
                    if let Some(Entry::Assistant {
                        stop_reason: slot,
                        error: err,
                        ..
                    }) = find_assistant(&mut entries, &id)
                    {
                        *slot = Some(stop_reason);
                        *err = error;
                    }
                }
                HarnessEvent::ToolCallStarted { id, name, args, .. } => {
                    entries.push(Entry::Tool {
                        id,
                        name,
                        args,
                        result: None,
                    });
                }
                HarnessEvent::ToolCallEnded { id, result, .. } => {
                    if let Some(Entry::Tool { result: slot, .. }) = find_tool(&mut entries, &id) {
                        *slot = Some(result);
                    }
                }
                HarnessEvent::StatusChanged { status: s, .. } => status = s,
                HarnessEvent::SessionSnapshot { .. } | HarnessEvent::Error { .. } => {}
            }
        }
        (entries, status)
    }

    /// Whether the recording holds no events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

fn find_assistant<'a>(entries: &'a mut [Entry], id: &MessageId) -> Option<&'a mut Entry> {
    entries
        .iter_mut()
        .rev()
        .find(|e| matches!(e, Entry::Assistant { id: c, .. } if c == id))
}

fn find_tool<'a>(entries: &'a mut [Entry], id: &ToolCallId) -> Option<&'a mut Entry> {
    entries
        .iter_mut()
        .rev()
        .find(|e| matches!(e, Entry::Tool { id: c, .. } if c == id))
}

/// Serves a [`Recording`] to whichever UI connects.
pub struct FakeHarness {
    recording: Recording,
    /// Delay between events, so streaming looks like streaming.
    pace: Duration,
}

impl FakeHarness {
    /// Serve this recording, pacing events by `pace`.
    #[must_use]
    pub fn new(recording: Recording, pace: Duration) -> Self {
        Self { recording, pace }
    }

    /// Accept connections until cancelled.
    ///
    /// One connection is served at a time: a second UI is a real scenario for the daemon and
    /// not one a replay tool needs to model.
    pub async fn serve(&self, listener: UnixListener) -> Result<(), IpcError> {
        loop {
            let (stream, _) = listener.accept().await?;
            let cred = PeerCred::of(&stream)?;
            if !cred.is_same_user() {
                continue;
            }
            // A UI that dies mid-replay is expected, not exceptional: it is the detach path.
            if let Err(e) = self.session(stream).await
                && !matches!(e, IpcError::Disconnected)
            {
                return Err(e);
            }
        }
    }

    async fn session(&self, stream: UnixStream) -> Result<(), IpcError> {
        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);

        let from = match reader.read::<UiCommand>().await? {
            UiCommand::Attach { from_cursor, .. } => from_cursor,
            _ => Cursor::ZERO,
        };

        let (entries, status) = self.recording.snapshot_at(from);
        writer
            .write(&HarnessEvent::SessionSnapshot {
                cursor: from,
                session: SessionId::new("replay"),
                entries,
                status,
            })
            .await?;

        for event in self.recording.events().iter().filter(|e| e.cursor() > from) {
            writer.write(event).await?;
            if !self.pace.is_zero() {
                tokio::time::sleep(self.pace).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# a comment line is ignored
{"event":"user_message","cursor":1,"id":"m1","text":"hi"}

{"event":"assistant_started","cursor":2,"id":"a1"}
{"event":"assistant_delta","cursor":3,"id":"a1","text":"hello","thinking":""}
"#;

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let recording = Recording::parse(SAMPLE).expect("parse");
        assert_eq!(recording.len(), 3);
    }

    #[test]
    fn events_keep_their_recorded_cursors() {
        let recording = Recording::parse(SAMPLE).expect("parse");
        let cursors: Vec<u64> = recording.events().iter().map(|e| e.cursor().0).collect();
        assert_eq!(cursors, [1, 2, 3]);
    }

    #[test]
    fn an_empty_recording_is_empty() {
        let recording = Recording::parse("").expect("parse");
        assert!(recording.is_empty());
    }

    #[tokio::test]
    async fn a_ui_receives_a_snapshot_then_the_recorded_events() {
        let dir = std::env::temp_dir().join(format!("axum-replay-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let path = dir.join("test.sock");
        let listener = axum_ipc::bind(&path).await.expect("bind");

        let recording = Recording::parse(SAMPLE).expect("parse");
        let harness = FakeHarness::new(recording, Duration::ZERO);
        tokio::spawn(async move { harness.serve(listener).await });

        let stream = axum_ipc::connect(&path).await.expect("connect");
        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);

        writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: Cursor::ZERO,
            })
            .await
            .expect("attach");

        let first: HarnessEvent = reader.read().await.expect("snapshot");
        assert!(matches!(first, HarnessEvent::SessionSnapshot { .. }));

        for expected in [1_u64, 2, 3] {
            let event: HarnessEvent = reader.read().await.expect("event");
            assert_eq!(event.cursor().0, expected);
        }

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[test]
    fn a_snapshot_folds_the_events_at_or_before_the_cursor() {
        let recording = Recording::parse(SAMPLE).expect("parse");
        let (entries, _) = recording.snapshot_at(Cursor(3));
        assert_eq!(entries.len(), 2, "one user message, one assistant message");
        match &entries[1] {
            Entry::Assistant { text, .. } => assert_eq!(text, "hello"),
            other => panic!("expected an assistant entry, got {other:?}"),
        }
    }

    #[test]
    fn a_snapshot_at_zero_is_empty() {
        let recording = Recording::parse(SAMPLE).expect("parse");
        let (entries, status) = recording.snapshot_at(Cursor::ZERO);
        assert!(entries.is_empty());
        assert_eq!(status, AgentStatus::Idle);
    }

    #[tokio::test]
    async fn attaching_with_a_cursor_replays_only_what_follows() {
        let dir = std::env::temp_dir().join(format!("axum-resume-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let path = dir.join("test.sock");
        let listener = axum_ipc::bind(&path).await.expect("bind");

        let recording = Recording::parse(SAMPLE).expect("parse");
        let harness = FakeHarness::new(recording, Duration::ZERO);
        tokio::spawn(async move { harness.serve(listener).await });

        let stream = axum_ipc::connect(&path).await.expect("connect");
        let (read_half, write_half) = stream.into_split();
        let mut reader = FrameReader::new(read_half);
        let mut writer = FrameWriter::new(write_half);

        writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: Cursor(2),
            })
            .await
            .expect("attach");

        // Cold attach: the history before the cursor arrives as entries, not as replayed
        // events, and only the remainder streams.
        let snapshot: HarnessEvent = reader.read().await.expect("snapshot");
        match snapshot {
            HarnessEvent::SessionSnapshot { entries, .. } => {
                assert_eq!(entries.len(), 2, "the user message and the started reply");
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }

        let event: HarnessEvent = reader.read().await.expect("event");
        assert_eq!(event.cursor().0, 3, "only events past the cursor replay");

        tokio::fs::remove_dir_all(&dir).await.ok();
    }
}
