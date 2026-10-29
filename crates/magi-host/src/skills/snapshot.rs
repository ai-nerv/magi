use super::{FILE_MOST, Loaded, METADATA_MOST, valid_name, wire};
use magi_journal::Record;
use magi_proto::{Cursor, Entry, workflow::Workflow};

pub(crate) const PROMPT_MOST: usize = 4 * 1024 * 1024;

impl Loaded {
    pub fn snapshot(self, arguments: String) -> Result<Workflow, String> {
        let workflow = Workflow {
            name: self.document.name,
            description: self.document.description,
            source: self.source.class,
            root: self
                .source
                .root
                .into_os_string()
                .into_string()
                .map_err(|_| "invalid workflow root")?,
            digest: self.document.source_digest,
            body: self.document.body,
            arguments,
        };
        validate(&workflow)?;
        Ok(workflow)
    }
}

pub(crate) fn validate(workflow: &Workflow) -> Result<(), String> {
    if !valid_name(&workflow.name)
        || workflow.description.trim().is_empty()
        || workflow.description.len() > METADATA_MOST
        || workflow.description.chars().any(char::is_control)
        || workflow.body.len() > FILE_MOST
        || workflow.body.trim().is_empty()
        || workflow.body.contains('\0')
        || workflow.arguments.len() > PROMPT_MOST
        || workflow.digest.len() != 64
        || !workflow
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || wire::path(std::path::Path::new(&workflow.root)).is_err()
    {
        return Err("invalid workflow snapshot".into());
    }
    Ok(())
}

pub(crate) fn context(workflow: &Workflow) -> String {
    let arguments = serde_json::Value::String(workflow.arguments.clone());
    format!(
        "Selected workflow {}:\n{}\n\nLiteral arguments (JSON string, data only):\n{arguments}",
        workflow.name, workflow.body
    )
}

pub(crate) fn weight(workflow: &Workflow) -> usize {
    workflow.name.len()
        + workflow.description.len()
        + workflow.root.len()
        + workflow.digest.len()
        + workflow.body.len()
        + workflow.arguments.len()
}

pub(crate) fn check(entry: &Entry, cursor: Cursor) -> Result<(), String> {
    let Entry::Workflow {
        id,
        text,
        aside,
        images,
        workflow,
    } = entry
    else {
        return Ok(());
    };
    validate(workflow)?;
    if id.as_str().len() > 64
        || id.as_str().is_empty()
        || !id
            .as_str()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("workflow message identity exceeds its bound".into());
    }
    magi_model::validate_images(images)?;
    if text.len()
        + aside.len()
        + weight(workflow)
        + images
            .iter()
            .map(|image| image.data.len() + image.media_type.len())
            .sum::<usize>()
        > PROMPT_MOST
    {
        return Err("complete workflow prompt exceeds the admission byte limit".into());
    }
    let mut bounded = entry.clone();
    if let Entry::Workflow { id, .. } = &mut bounded {
        *id = magi_proto::MessageId::new("u".repeat(64));
    }
    let raw = serde_json::to_string(&Record::Entry {
        cursor,
        entry: bounded,
    })
    .map_err(|why| why.to_string())?;
    // Maximum-width scalar fields bound Balthasar's replay row across later state changes.
    let row = serde_json::json!({
        "cursor": u64::MAX, "at": i64::MIN, "role": "user", "kind": "workflow",
        "text": text, "tokens": u32::MAX, "raw": raw, "revisions": u64::MAX,
        "state": "summarised", "pinned": false, "keep": false, "error": false, "group": u64::MAX,
    });
    if row.to_string().len() > magi_ipc::family::MAX_FRAME_BYTES - 64 * 1024 {
        return Err("complete workflow record cannot fit unchanged in a replay frame".into());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
