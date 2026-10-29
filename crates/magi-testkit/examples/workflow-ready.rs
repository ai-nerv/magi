//! Read-only, bounded session readiness probe for terminal acceptance.

use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{AgentStatus, Cursor, HarnessEvent, UiCommand};
use std::io::Write;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket = std::env::args().nth(1).ok_or("socket required")?;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let stream = magi_ipc::connect(std::path::Path::new(&socket)).await?;
        let (read, write) = stream.into_split();
        let mut reader = FrameReader::new(read);
        let mut writer = FrameWriter::new(write);
        writer
            .write(&UiCommand::Attach {
                session: None,
                from_cursor: Cursor(u64::MAX),
                draws: false,
                key: None,
            })
            .await?;
        for _ in 0..4096 {
            let event: HarnessEvent = reader.read().await?;
            match event {
                HarnessEvent::SessionSnapshot { status, .. }
                | HarnessEvent::StatusChanged { status, .. } => {
                    println!("{}", serde_json::to_string(&status)?);
                    std::io::stdout().flush()?;
                    if status == AgentStatus::Idle {
                        return Ok::<(), Box<dyn std::error::Error>>(());
                    }
                }
                HarnessEvent::Refused { message, .. } => return Err(message.into()),
                _ => {}
            }
        }
        Err("readiness event limit exceeded".into())
    })
    .await??;
    Ok(())
}
