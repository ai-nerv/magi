//! A screen with nobody at it: attach to a running session, say what it is told to, and print every
//! event that comes back, one JSON object to a line. The socket and the framing are production
//! code, so two of these are two real clients of one real session.

use magi_ipc::{FrameReader, FrameWriter};
use magi_proto::{Cursor, HarnessEvent, StopReason, UiCommand};
use std::collections::BTreeMap;

fn flags() -> BTreeMap<String, String> {
    parse_flags(std::env::args().skip(1))
}

fn parse_flags(args: impl IntoIterator<Item = String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(name) = arg.strip_prefix("--") {
            let value = if matches!(name, "from-end" | "successful-answers") {
                "true".to_owned()
            } else {
                args.next().unwrap_or_default()
            };
            out.insert(name.to_owned(), value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_flags;

    #[test]
    fn from_end_does_not_consume_the_following_prompt_flag() {
        let flags = parse_flags(
            [
                "--socket",
                "/tmp/s.sock",
                "--from-end",
                "--prompt",
                "later",
                "--answers",
                "1",
            ]
            .map(str::to_owned),
        );
        assert!(flags.contains_key("from-end"));
        assert_eq!(flags.get("prompt").map(String::as_str), Some("later"));
        assert_eq!(flags.get("answers").map(String::as_str), Some("1"));
    }

    #[test]
    fn a_dash_prefixed_prompt_is_a_value_not_a_boolean_flag() {
        let flags = parse_flags(["--prompt", "--from-end", "--seconds", "2"].map(str::to_owned));
        assert_eq!(flags.get("prompt").map(String::as_str), Some("--from-end"));
        assert!(!flags.contains_key("from-end"));
        assert_eq!(flags.get("seconds").map(String::as_str), Some("2"));
    }

    #[test]
    fn successful_answers_does_not_consume_the_following_prompt() {
        let flags = parse_flags(["--successful-answers", "--prompt", "recover"].map(str::to_owned));
        assert!(flags.contains_key("successful-answers"));
        assert_eq!(flags.get("prompt").map(String::as_str), Some("recover"));
    }

    #[test]
    fn inline_image_flags_preserve_bytes_and_refuse_invalid_attachments() {
        let mut flags = parse_flags(["--image", "iVBORw0KGgo="].map(str::to_owned));
        let images = super::images(&flags).expect("image");
        assert_eq!(images[0].data, "iVBORw0KGgo=");
        assert_eq!(images[0].media_type, "image/png");
        flags.insert("image-type".into(), "text/plain".into());
        assert!(super::images(&flags).is_err());
        assert!(
            super::images(&Default::default())
                .expect("no image")
                .is_empty()
        );
    }
}

fn images(flags: &BTreeMap<String, String>) -> Result<Vec<magi_model::Image>, String> {
    let images = flags.get("image").map_or_else(Vec::new, |data| {
        vec![magi_model::Image {
            data: data.clone(),
            media_type: flags
                .get("image-type")
                .cloned()
                .unwrap_or_else(|| "image/png".into()),
        }]
    });
    magi_model::validate_images(&images)?;
    Ok(images)
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
            key: magi_ipc::read_key(std::path::Path::new(socket)),
        })
        .await?;
    if let Some(id) = flags.get("resume") {
        let resuming = async {
            loop {
                let event: HarnessEvent = reader.read().await?;
                println!("{}", serde_json::to_string(&event)?);
                match event {
                    HarnessEvent::SessionSnapshot { .. } => break,
                    HarnessEvent::Refused { message, .. } => return Err(message.into()),
                    _ => {}
                }
            }
            writer.write(&UiCommand::Resume { id: id.clone() }).await?;
            loop {
                let event: HarnessEvent = reader.read().await?;
                println!("{}", serde_json::to_string(&event)?);
                match event {
                    HarnessEvent::SessionSnapshot { session, .. } if session.as_str() == id => {
                        return Ok::<(), Box<dyn std::error::Error>>(());
                    }
                    HarnessEvent::Refused { message, .. } => return Err(message.into()),
                    _ => {}
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(patience), resuming).await??;
    }
    if let Some(text) = flags.get("prompt") {
        writer
            .write(&UiCommand::SubmitPrompt {
                images: images(&flags)?,
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
                && (!flags.contains_key("successful-answers") || *stop_reason != StopReason::Error)
            {
                ended += 1;
            }
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(patience), reading).await;
    let _ = writer.write(&UiCommand::Detach).await;
    if flags.contains_key("successful-answers") {
        outcome??;
    }
    Ok(())
}
