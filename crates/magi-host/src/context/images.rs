use super::*;
use magi_proto::{MessageId, ToolCallId, ToolResult};

#[test]
fn images_survive_history_reconstruction_and_elision_is_explicit() {
    let image = magi_model::Image {
        data: "pixels".into(),
        media_type: "image/png".into(),
    };
    let entries = [
        Entry::User {
            id: MessageId::new("user"),
            text: "inspect @photo.png".into(),
            aside: String::new(),
            images: vec![image.clone()],
        },
        Entry::Assistant {
            id: MessageId::new("assistant"),
            text: String::new(),
            thinking: String::new(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            signatures: Default::default(),
            usage: Default::default(),
        },
        Entry::Tool {
            id: ToolCallId::new("read"),
            name: "read".into(),
            args: "{}".into(),
            thought_signature: None,
            result: Some(ToolResult {
                output: "image photo.png".into(),
                is_error: false,
                shown: None,
                images: vec![image],
            }),
        },
    ];
    let rebuilt = of_entries(&entries);
    assert!(
        matches!(&rebuilt.messages[0].content[1], Content::Image { data, .. } if data == "pixels")
    );
    assert!(
        matches!(&rebuilt.messages[2].content[0], Content::ToolResult { images, .. } if images.len() == 1)
    );
    let mut stubbed = Built::default();
    stubbed.entry(&entries[0], None);
    stubbed.entry(&entries[1], None);
    stubbed.entry(&entries[2], Some("image photo.png; read photo.png to view"));
    let stubbed = stubbed.finish();
    assert!(
        matches!(&stubbed.messages[2].content[0], Content::ToolResult { images, content, .. }
        if images.is_empty() && content.contains("read photo.png"))
    );
    assert!(
        matches!(&rebuilt.messages[2].content[0], Content::ToolResult { images, .. } if images.len() == 1)
    );
}
