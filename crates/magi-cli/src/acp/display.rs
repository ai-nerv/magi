use magi_proto::HarnessEvent;
use serde_json::{Value, json};

#[derive(Clone, Copy, Default)]
pub(super) struct Policy {
    pub compaction: bool,
}

impl Policy {
    pub(super) fn boundary(self, session: &str, event: &HarnessEvent) -> Option<Value> {
        match event {
            HarnessEvent::Compacted { id, summary, .. } if self.compaction => Some(notification(
                session,
                json!({
                    "sessionUpdate":"compaction_update",
                    "compactionId":format!("magi-compaction-{id}"),
                    "status":"completed",
                    "summary":if summary.is_empty() { vec![] } else {
                        vec![json!({"type":"text", "text":summary})]
                    }
                }),
            )),
            HarnessEvent::Compacted { summary, .. } => Some(chunk(
                session,
                "agent_message_chunk",
                &if summary.is_empty() {
                    "Context compacted.".to_owned()
                } else {
                    format!("Context compacted. Retained summary:\n\n{summary}")
                },
            )),
            HarnessEvent::Branched { keeps, .. } => Some(chunk(
                session,
                "agent_message_chunk",
                &format!(
                    "Context rewound to the first {keeps} entries. Earlier conversation remains in history."
                ),
            )),
            _ => None,
        }
    }
}

pub(super) fn message(
    session: &str,
    kind: &str,
    text: &str,
    images: Vec<magi_model::Image>,
) -> Vec<Value> {
    let mut frames = vec![chunk(session, kind, text)];
    frames.extend(images.into_iter().map(|image| {
        notification(
            session,
            json!({
                "sessionUpdate":kind,
                "content":{"type":"image", "mimeType":image.media_type, "data":image.data}
            }),
        )
    }));
    frames
}

pub(super) fn chunk(session: &str, kind: &str, text: &str) -> Value {
    notification(
        session,
        json!({"sessionUpdate":kind, "content":{"type":"text", "text":text}}),
    )
}

pub(super) fn notification(session: &str, update: Value) -> Value {
    json!({"jsonrpc":"2.0", "method":"session/update", "params":{"sessionId":session, "update":update}})
}

#[cfg(test)]
mod tests;
