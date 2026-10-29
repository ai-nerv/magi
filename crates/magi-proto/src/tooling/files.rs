use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    Read,
    Edit,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCapability {
    pub version: u16,
    pub operation: FileOperation,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileRevision {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "supplied"
    )]
    file_revision: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "supplied"
    )]
    file_recovery: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "supplied"
    )]
    revision_contract: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "supplied"
    )]
    expected_revision: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "supplied"
    )]
    mutation_id: Option<Value>,
}

impl FileRevision {
    pub fn native(
        capability: &FileCapability,
        guard: Option<(&str, &str)>,
    ) -> Result<Self, String> {
        let mut revision = Self {
            file_revision: Some(serde_json::to_value(capability).map_err(|why| why.to_string())?),
            ..Self::default()
        };
        if let Some((expected, id)) = guard {
            revision.revision_contract = Some(Value::from(1));
            revision.expected_revision = Some(Value::from(expected));
            revision.mutation_id = Some(Value::from(id));
        }
        revision.validate()?;
        Ok(revision)
    }

    #[must_use]
    pub fn mutation_id(&self) -> Option<&str> {
        self.mutation_id.as_ref().and_then(Value::as_str)
    }

    #[must_use]
    pub fn capability(&self) -> Option<FileCapability> {
        self.file_revision
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    #[must_use]
    pub fn requested(&self) -> bool {
        self.file_revision.is_some()
            || self.file_recovery.is_some()
            || self.revision_contract.is_some()
            || self.expected_revision.is_some()
            || self.mutation_id.is_some()
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.requested() {
            return Ok(());
        }
        let capability: FileCapability = serde_json::from_value(
            self.file_revision
                .as_ref()
                .ok_or("native file admission requires a positive capability")?
                .clone(),
        )
        .map_err(|_| "native file admission is incomplete or unsupported".to_owned())?;
        if capability.version != 1 {
            return Err("native file admission version is unsupported".into());
        }
        if self.file_recovery.is_some()
            && (self.file_recovery.as_ref().and_then(Value::as_u64) != Some(1)
                || capability.operation != FileOperation::Read)
        {
            return Err("file recovery requires supported version1 native read admission".into());
        }
        let mutation = self.revision_contract.is_some()
            || self.expected_revision.is_some()
            || self.mutation_id.is_some();
        if capability.operation == FileOperation::Read {
            return if mutation {
                Err("native file read admission cannot carry mutation guards".into())
            } else {
                Ok(())
            };
        }
        let expected = self.expected_revision.as_ref().and_then(Value::as_str);
        let id = self.mutation_id.as_ref().and_then(Value::as_str);
        if self.revision_contract.as_ref().and_then(Value::as_u64) != Some(1)
            || !expected.is_some_and(|value| value == "absent" || receipt(value))
            || !id.is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= 128
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            })
        {
            return Err("native file mutation requires version1, a supported revision and bounded logical id".into());
        }
        Ok(())
    }

    pub fn recovery(mut self) -> Result<Self, String> {
        self.file_recovery = Some(Value::from(1));
        self.validate()?;
        Ok(self)
    }

    #[must_use]
    pub fn recovering(&self) -> bool {
        self.file_recovery.as_ref().and_then(Value::as_u64) == Some(1)
    }
}

#[must_use]
pub fn receipt(value: &str) -> bool {
    value.strip_prefix("cfr1:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn supplied<'de, D: Deserializer<'de>>(input: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(input).map(Some)
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absence: Option<FileAbsence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_contract: Option<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observations: Vec<FileObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutation: Option<FileMutation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileAbsence {
    pub version: u16,
    pub path_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileObservation {
    pub path_key: String,
    pub revision: String,
    pub coverage: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMutation {
    pub id: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<String>,
}
