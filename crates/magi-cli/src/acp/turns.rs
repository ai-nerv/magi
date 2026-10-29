use std::time::Duration;

use agent_client_protocol_schema::v1::{Error, PromptResponse, RequestId};
use anyhow::{Result, bail};
use magi_proto::{HarnessEvent, MessageId, StopReason};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{
    display::{self, chunk, notification},
    envelope,
    intake::Item,
};

mod stream;
pub(super) use stream::Stream;

pub(super) struct Turn {
    _lease: Option<Item>,
    id: Option<RequestId>,
    admitted: bool,
    assistant: Option<MessageId>,
    reason: Option<StopReason>,
    failed: bool,
    unsupported: bool,
    cancelled: bool,
    pub deadline: Option<Instant>,
}

pub(super) struct Update {
    pub frames: Vec<Value>,
    pub complete: Option<Value>,
    pub interrupt: bool,
}

impl Turn {
    pub(super) fn new(id: RequestId, lease: Item) -> Self {
        Self {
            _lease: Some(lease),
            id: Some(id),
            admitted: false,
            assistant: None,
            reason: None,
            failed: false,
            unsupported: false,
            cancelled: false,
            deadline: Some(Instant::now() + Duration::from_secs(75)),
        }
    }

    pub(super) fn background() -> Self {
        Self {
            _lease: None,
            id: None,
            admitted: true,
            assistant: None,
            reason: None,
            failed: false,
            unsupported: false,
            cancelled: false,
            deadline: None,
        }
    }

    pub(super) fn finish(&self, session: &str) -> Update {
        let notice = if self.id.is_some() {
            None
        } else if self.cancelled {
            Some("Automatic native turn cancelled")
        } else if self.unsupported {
            Some("Automatic native turn stopped: interactive operation is unsupported")
        } else if self.failed {
            Some("Automatic native turn failed")
        } else {
            None
        };
        Update {
            frames: notice
                .into_iter()
                .map(|text| chunk(session, "agent_message_chunk", text))
                .collect(),
            complete: self.id.as_ref().map(|id| self.response(id)),
            interrupt: false,
        }
    }

    pub(super) fn cancel(&mut self) -> bool {
        if self.cancelled {
            return false;
        }
        self.cancelled = true;
        self.deadline = Some(Instant::now() + Duration::from_secs(30));
        true
    }

    pub(super) fn cancelled(&self) -> bool {
        self.cancelled || self.unsupported
    }

    pub(super) fn event(&mut self, session: &str, event: HarnessEvent) -> Result<Update> {
        let mut update = Update {
            frames: Vec::new(),
            complete: None,
            interrupt: false,
        };
        match event {
            HarnessEvent::UserMessage { text, images, .. }
            | HarnessEvent::WorkflowMessage { text, images, .. } => {
                if self.admitted && self.id.is_some() {
                    bail!("unexpected second native prompt admission");
                }
                self.admitted = true;
                if !self.cancelled {
                    self.deadline = None;
                }
                update.frames = display::message(session, "user_message_chunk", &text, images);
            }
            HarnessEvent::MessageArrived {
                who,
                kin,
                sort,
                text,
                ..
            } => {
                update.frames.push(chunk(
                    session,
                    "user_message_chunk",
                    &format!("[{kin}:{who} {sort}]\n{text}"),
                ));
            }
            HarnessEvent::AssistantStarted { id, .. } => {
                self.assistant = Some(id);
                self.failed = false;
                self.reason = None;
            }
            HarnessEvent::AssistantDelta {
                id, text, thinking, ..
            } => {
                if self.assistant.as_ref() != Some(&id) || !self.admitted {
                    bail!("uncorrelated native assistant delta");
                }
                for (kind, text) in [
                    ("agent_message_chunk", text),
                    ("agent_thought_chunk", thinking),
                ] {
                    if !text.is_empty() {
                        update.frames.push(chunk(session, kind, &text));
                    }
                }
            }
            HarnessEvent::AssistantEnded {
                id,
                stop_reason,
                error,
                ..
            } => {
                if self.assistant.as_ref().is_some_and(|active| active != &id) {
                    bail!("uncorrelated native assistant completion");
                }
                self.reason = Some(stop_reason);
                self.failed = error.is_some() || stop_reason == StopReason::Error;
                self.assistant = None;
            }
            HarnessEvent::ToolCallStarted { id, name, args, .. } => {
                update.frames.push(notification(
                    session,
                    json!({
                        "sessionUpdate":"tool_call", "toolCallId":format!("magi-tool-{id}"), "title":name,
                        "status":"in_progress", "rawInput":args
                    }),
                ));
            }
            HarnessEvent::ToolCallEnded { id, result, .. } => {
                let mut content = vec![json!({
                    "type":"content", "content":{"type":"text", "text":result.output}
                })];
                for image in result.images {
                    content.push(json!({"type":"content", "content":{
                        "type":"image", "mimeType":image.media_type, "data":image.data
                    }}));
                }
                update.frames.push(notification(
                    session,
                    json!({
                        "sessionUpdate":"tool_call_update", "toolCallId":format!("magi-tool-{id}"),
                        "status":if result.is_error { "failed" } else { "completed" },
                        "content":content
                    }),
                ));
            }
            HarnessEvent::PermissionAsked { .. } => bail!("unhandled native approval"),
            HarnessEvent::Asked { .. } | HarnessEvent::Surfaced { .. } => {
                self.unsupported = true;
                self.deadline = Some(Instant::now() + Duration::from_secs(30));
                update.interrupt = true;
            }
            HarnessEvent::PromptRefused { .. } => {
                self.failed = true;
                update = self.finish(session);
            }
            HarnessEvent::Refused { .. } => bail!("uncorrelated native refusal"),
            HarnessEvent::Error { .. } => self.failed = true,
            HarnessEvent::SessionSnapshot { .. } => {
                bail!("native stream requires resynchronization")
            }
            _ => {}
        }
        Ok(update)
    }

    fn response(&self, id: &RequestId) -> Value {
        use agent_client_protocol_schema::v1::StopReason as AcpStop;
        let reason = if self.cancelled {
            AcpStop::Cancelled
        } else if self.unsupported {
            return envelope::failure(
                Some(id.clone()),
                Error::new(
                    -32000,
                    "Native interactive operation is not supported by this adapter yet",
                ),
            );
        } else if self.failed {
            return envelope::failure(Some(id.clone()), Error::new(-32000, "Native prompt failed"));
        } else {
            match self.reason {
                Some(StopReason::EndTurn) => AcpStop::EndTurn,
                Some(StopReason::Length) => AcpStop::MaxTokens,
                _ => {
                    return envelope::failure(
                        Some(id.clone()),
                        Error::new(-32000, "Native turn ended without a successful result"),
                    );
                }
            }
        };
        json!({"jsonrpc":"2.0", "id":id, "result":PromptResponse::new(reason)})
    }
}

#[cfg(test)]
mod tests;
