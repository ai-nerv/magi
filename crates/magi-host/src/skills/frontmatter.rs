use super::{Document, FILE_MOST, Failure, METADATA_MOST, Metadata, valid_name};
use sha2::{Digest, Sha256};

pub fn parse(directory: &str, bytes: &[u8]) -> Result<Document, Failure> {
    if bytes.len() > FILE_MOST {
        return Err(Failure::Oversized);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Failure::Encoding)?;
    let (metadata, offset) = header(directory, text)?;
    let body = text.get(offset..).ok_or(Failure::Metadata)?;
    if body.trim().is_empty() || body.contains('\0') {
        return Err(Failure::Body);
    }
    Ok(Document {
        name: metadata.name,
        description: metadata.description,
        body: body.to_owned(),
        source_digest: format!("{:x}", Sha256::digest(bytes)),
    })
}

pub fn metadata(directory: &str, bytes: &[u8]) -> Result<Metadata, Failure> {
    if bytes.len() > METADATA_MOST {
        return Err(Failure::MetadataOversized);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Failure::Encoding)?;
    let (metadata, offset) = header(directory, text)?;
    if offset != bytes.len() {
        return Err(Failure::Metadata);
    }
    Ok(metadata)
}

fn header(directory: &str, text: &str) -> Result<(Metadata, usize), Failure> {
    if !valid_name(directory) {
        return Err(Failure::Name);
    }
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().ok_or(Failure::Metadata)?;
    if line(first) != "---" {
        return Err(Failure::Metadata);
    }
    let mut offset = first.len();
    let mut name = None;
    let mut description = None;
    let mut closed = false;
    for raw in lines {
        offset += raw.len();
        if offset > METADATA_MOST {
            return Err(Failure::MetadataOversized);
        }
        let value = line(raw);
        if value == "---" {
            closed = true;
            break;
        }
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let (key, value) = value.split_once(':').ok_or(Failure::Metadata)?;
        let slot = match key {
            "name" => &mut name,
            "description" => &mut description,
            _ => return Err(Failure::Metadata),
        };
        if slot.is_some() {
            return Err(Failure::Metadata);
        }
        *slot = Some(scalar(value.trim())?);
    }
    if !closed {
        return Err(Failure::Metadata);
    }
    let name = name.ok_or(Failure::Metadata)?;
    let description = description.ok_or(Failure::Metadata)?;
    if name != directory || !valid_name(&name) {
        return Err(Failure::Name);
    }
    Ok((Metadata { name, description }, offset))
}

fn line(raw: &str) -> &str {
    let raw = raw.strip_suffix('\n').unwrap_or(raw);
    raw.strip_suffix('\r').unwrap_or(raw)
}

fn scalar(value: &str) -> Result<String, Failure> {
    let parsed = if value.starts_with('"') {
        serde_json::from_str::<String>(value).map_err(|_| Failure::Metadata)?
    } else if let Some(inside) = value.strip_prefix('\'') {
        let inside = inside.strip_suffix('\'').ok_or(Failure::Metadata)?;
        let mut chars = inside.chars();
        let mut parsed = String::new();
        while let Some(ch) = chars.next() {
            if ch == '\'' && chars.next() != Some('\'') {
                return Err(Failure::Metadata);
            }
            parsed.push(ch);
        }
        parsed
    } else {
        if value.starts_with([
            '!', '&', '*', '{', '}', '[', ']', '|', '>', '%', '@', '`', '#', '?', '-',
        ]) || value.contains(": ")
            || value.contains(" #")
        {
            return Err(Failure::Metadata);
        }
        value.to_owned()
    };
    if parsed.trim().is_empty() || parsed.chars().any(char::is_control) {
        return Err(Failure::Metadata);
    }
    Ok(parsed)
}
