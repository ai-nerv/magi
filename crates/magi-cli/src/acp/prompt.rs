use agent_client_protocol_schema::v1::{Error, PromptRequest};
use magi_proto::UiCommand;
use serde_json::Value;

const BYTES: usize = 1024 * 1024;

pub(super) struct Prompt {
    pub session: String,
    pub command: UiCommand,
}

pub(super) fn parse(value: Value) -> Result<Prompt, Error> {
    let invalid = Error::invalid_params;
    let object = value.as_object().ok_or_else(invalid)?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "sessionId" | "prompt" | "_meta"))
    {
        return Err(invalid());
    }
    let blocks = object
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if blocks.is_empty() || blocks.len() > 64 {
        return Err(invalid());
    }
    let mut text = String::new();
    let mut resources = Vec::new();
    for block in blocks {
        let fields = block.as_object().ok_or_else(invalid)?;
        match fields.get("type").and_then(Value::as_str) {
            Some("text") => {
                if fields
                    .keys()
                    .any(|key| !matches!(key.as_str(), "type" | "text" | "annotations" | "_meta"))
                {
                    return Err(invalid());
                }
                let part = fields
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(part);
            }
            Some("resource_link") => {
                if fields.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "type"
                            | "uri"
                            | "name"
                            | "title"
                            | "description"
                            | "mimeType"
                            | "size"
                            | "annotations"
                            | "_meta"
                    )
                }) {
                    return Err(invalid());
                }
                let uri = fields
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                if uri.is_empty() || uri.len() > 4096 || uri.chars().any(char::is_control) {
                    return Err(invalid());
                }
                resources.push(block.clone());
            }
            _ => return Err(Error::new(-32602, "Unsupported prompt content type")),
        }
    }
    if serde_json::to_vec(&value).map_err(|_| invalid())?.len() > BYTES {
        return Err(invalid());
    }
    let request: PromptRequest = serde_json::from_value(value).map_err(|_| invalid())?;
    let session = request.session_id.to_string();
    if super::history::validate_id(&session).is_err() {
        return Err(invalid());
    }
    let aside = if resources.is_empty() {
        String::new()
    } else {
        format!(
            "User-supplied resource references (metadata, not fetched content):\n{}",
            serde_json::to_string(&resources).map_err(|_| invalid())?
        )
    };
    Ok(Prompt {
        session,
        command: UiCommand::SubmitPrompt {
            text,
            aside,
            images: Vec::new(),
        },
    })
}

#[cfg(test)]
mod tests;
