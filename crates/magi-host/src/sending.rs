//! Writing an event to a screen. One too large for a frame is cut down and sent, rather than
//! refused whole, which would take the connection with it and meet the same event on reattach.

use magi_ipc::{FrameWriter, IpcError};
use magi_proto::HarnessEvent;
use tokio::io::AsyncWrite;

/// The widest text a cut-down event keeps first; each try that still does not fit keeps a quarter.
const FIRST: usize = 1024 * 1024;

/// Strings this short are never cut: event tags and names live among them.
const SHORT: usize = 256;

/// Write `event`, cutting its texts until it fits in one frame.
///
/// # Errors
/// When the connection fails.
pub async fn send<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    event: &HarnessEvent,
) -> Result<(), IpcError> {
    let len = match writer.write(event).await {
        Err(IpcError::FrameTooLarge { len }) => len,
        result => return result,
    };
    magi_model::noted!("ui: a {len} byte event is cut down to fit a frame");
    let whole = serde_json::to_value(event).unwrap_or_default();
    let mut keep = FIRST;
    while keep >= SHORT {
        let mut cut = whole.clone();
        shorten(&mut cut, keep);
        if let Ok(fitted) = serde_json::from_value::<HarnessEvent>(cut) {
            match writer.write(&fitted).await {
                Err(IpcError::FrameTooLarge { .. }) => {}
                done => return done,
            }
        }
        keep /= 4;
    }
    writer
        .write(&HarnessEvent::Noticed {
            cursor: event.cursor(),
            text: format!("An event of {len} bytes was too large to show and was left out."),
        })
        .await
}

/// One screen's half of a connection, every event to it going through [`send`] within a deadline.
pub(crate) struct Screen<W> {
    writer: FrameWriter<W>,
    within: std::time::Duration,
}

impl<W: AsyncWrite + Unpin> Screen<W> {
    pub(crate) fn within(writer: FrameWriter<W>, within: std::time::Duration) -> Self {
        Self { writer, within }
    }

    /// # Errors
    /// When the connection fails, or the screen has not taken the event within the deadline.
    pub(crate) async fn write(&mut self, event: &HarnessEvent) -> Result<(), IpcError> {
        tokio::time::timeout(self.within, send(&mut self.writer, event))
            .await
            .map_err(|_| IpcError::Io(std::io::ErrorKind::TimedOut.into()))?
    }
}
/// Cut every string in `value` longer than `keep` bytes, marking where.
fn shorten(value: &mut serde_json::Value, keep: usize) {
    match value {
        serde_json::Value::String(text) => crate::scribe::cut(text, keep),
        serde_json::Value::Array(items) => items.iter_mut().for_each(|item| shorten(item, keep)),
        serde_json::Value::Object(fields) => {
            fields.values_mut().for_each(|field| shorten(field, keep));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::{Cursor, MessageId};

    #[tokio::test]
    async fn an_event_past_the_frame_cap_arrives_cut_down_and_the_stream_stays_in_step() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let mut writer = FrameWriter::new(client);
        let mut reader = magi_ipc::FrameReader::new(server);
        let huge = HarnessEvent::UserMessage {
            cursor: Cursor(3),
            id: MessageId::new("u3"),
            text: "y".repeat(magi_ipc::MAX_FRAME_BYTES + 1),
            images: Vec::new(),
        };
        let reading = tokio::spawn(async move {
            let first: HarnessEvent = reader.read().await.expect("the cut event");
            let second: HarnessEvent = reader.read().await.expect("the next event");
            (first, second)
        });
        send(&mut writer, &huge).await.expect("sent");
        send(
            &mut writer,
            &HarnessEvent::Refused {
                cursor: Cursor(4),
                message: "next".into(),
            },
        )
        .await
        .expect("sent");
        let (first, second) = reading.await.expect("read");
        let HarnessEvent::UserMessage { cursor, text, .. } = first else {
            panic!("the same event, cut: {first:?}");
        };
        assert_eq!(cursor, Cursor(3));
        assert!(
            text.len() <= FIRST + 128 && text.contains("bytes cut here"),
            "{}",
            text.len()
        );
        assert!(matches!(second, HarnessEvent::Refused { .. }), "{second:?}");
    }

    #[tokio::test]
    async fn a_screen_that_stops_reading_is_given_up_on_at_the_deadline() {
        let (client, _never_read) = tokio::io::duplex(1024);
        let within = std::time::Duration::from_millis(200);
        let mut screen = Screen::within(FrameWriter::new(client), within);
        let event = HarnessEvent::UserMessage {
            cursor: Cursor(1),
            id: MessageId::new("u1"),
            text: "y".repeat(64 * 1024),
            images: Vec::new(),
        };
        let started = std::time::Instant::now();
        let given_up = screen.write(&event).await.expect_err("never taken");
        assert!(
            matches!(&given_up, IpcError::Io(why) if why.kind() == std::io::ErrorKind::TimedOut),
            "{given_up:?}"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_closed_screen_is_a_write_failure_not_a_successful_delivery() {
        use tokio::io::AsyncReadExt;
        for partial in [false, true] {
            let (client, mut server) = tokio::io::duplex(16);
            let closing = tokio::spawn(async move {
                if partial {
                    let mut prefix = [0; 4];
                    server.read_exact(&mut prefix).await.expect("frame prefix");
                }
                drop(server);
            });
            let event = HarnessEvent::UserMessage {
                cursor: Cursor(1),
                id: MessageId::new("u1"),
                text: "x".repeat(1024),
                images: vec![],
            };
            let result = send(&mut FrameWriter::new(client), &event).await;
            assert!(matches!(result, Err(IpcError::Io(_))), "{result:?}");
            closing.await.expect("closed");
        }
    }
}
