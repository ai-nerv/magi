//! A screen with nobody at it: attach to a running session, say what it is told to, and print every
//! event that comes back, one JSON object to a line. The socket and the framing are production
//! code, so two of these are two real clients of one real session.

use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, StopReason, UiCommand};
use std::collections::BTreeMap;

fn flags() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(name) = arg.strip_prefix("--") {
            out.insert(name.to_owned(), args.next().unwrap_or_default());
        }
    }
    out
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let flags = flags();
    let socket = flags.get("socket").ok_or("--socket <path>")?;
    // How many finished answers to wait for, and for how long at most.
    let answers: usize = flags.get("answers").map_or(1, |n| n.parse().unwrap_or(1));
    let patience = flags.get("seconds").map_or(60, |n| n.parse().unwrap_or(60));

    let stream = magi_ipc::connect(std::path::Path::new(socket)).await?;
    let (read, write) = stream.into_split();
    let (mut reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    writer
        .write(&UiCommand::Attach {
            session: None,
            from_cursor: Cursor(if flags.contains_key("from-end") {
                u64::MAX
            } else {
                0
            }),
            draws: false,
        })
        .await?;
    if let Some(text) = flags.get("prompt") {
        writer
            .write(&UiCommand::SubmitPrompt {
                text: text.clone(),
                aside: String::new(),
            })
            .await?;
    }
    // Something a busy session has to refuse rather than act on.
    if let Some(level) = flags.get("thinking") {
        writer
            .write(&UiCommand::SetThinking {
                level: level.clone(),
            })
            .await?;
    }

    let mut ended = 0;
    let reading = async {
        while ended < answers {
            let event: HarnessEvent = reader.read().await?;
            println!("{}", serde_json::to_string(&event)?);
            // Nobody is here to answer, and the session waits: what was not granted in advance
            // is refused, as `magi -p` does.
            if let HarnessEvent::PermissionAsked { id, .. } = &event {
                writer
                    .write(&UiCommand::Permit {
                        id: id.clone(),
                        decision: magi_proto::permit::Decision::Deny,
                    })
                    .await?;
            }
            if let HarnessEvent::AssistantEnded { stop_reason, .. } = &event
                && *stop_reason != StopReason::ToolUse
            {
                ended += 1;
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(patience), reading).await;
    let _ = writer.write(&UiCommand::Detach).await;
    Ok(())
}
