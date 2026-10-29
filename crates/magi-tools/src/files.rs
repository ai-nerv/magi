use magi_proto::tooling::{FileCapability, FileMetadata, FileOperation, FileRevision};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct Owner(Arc<()>);

impl Owner {
    #[must_use]
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub struct Subject<'a> {
    pub supplier: &'a str,
    pub cwd: &'a str,
    pub path: &'a str,
}

pub trait Revisions: Send + Sync {
    fn admit(&self, subject: Subject<'_>, capability: &FileCapability) -> Result<Grant, String>;
}

pub trait Acknowledge: Send + Sync {
    fn delivered(&self, metadata: &FileMetadata, failed: bool);
}

pub struct Grant {
    revision: FileRevision,
    operation: FileOperation,
    owner: Owner,
    cursor: u64,
    acknowledge: Arc<dyn Acknowledge>,
}

impl Grant {
    pub fn new(
        revision: FileRevision,
        operation: FileOperation,
        owner: Owner,
        cursor: u64,
        acknowledge: Arc<dyn Acknowledge>,
    ) -> Self {
        Self {
            revision,
            operation,
            owner,
            cursor,
            acknowledge,
        }
    }

    #[must_use]
    pub fn revision(&self) -> &FileRevision {
        &self.revision
    }

    pub(crate) fn complete(self, mut output: crate::Output) -> crate::Output {
        output.file_delivery = None;
        let empty = FileMetadata::default();
        let metadata = output.files.as_deref().unwrap_or(&empty);
        if metadata.absence.is_some() && !self.revision.recovering()
            || !valid(
                metadata,
                self.operation,
                self.revision.mutation_id(),
                output.is_error,
            )
        {
            output.is_error = true;
            output.content = format!(
                "Native file outcome metadata is incomplete or unsupported; reread before mutation. Operation not retried.\n{}",
                output.content
            );
            output.files = self.revision.mutation_id().map(|id| Box::new(unknown(id)));
            return output;
        }
        output.file_delivery = Some(Delivery(Arc::new(Completed {
            owner: self.owner,
            cursor: self.cursor,
            acknowledge: self.acknowledge,
            metadata: metadata.clone(),
            failed: output.is_error,
            delivered: std::sync::atomic::AtomicBool::new(false),
        })));
        output
    }

    pub(crate) fn failed(self, why: String) -> crate::Output {
        let mut output = crate::Output::error(why);
        if let Some(id) = self.revision.mutation_id() {
            output.files = Some(Box::new(unknown(id)));
            output.content = format!(
                "Native file mutation outcome is unknown; reread before another mutation. Not retried.\n{}",
                output.content
            );
        }
        self.complete(output)
    }

    pub(crate) fn refused(self, why: String) -> crate::Output {
        let mut output = crate::Output::error(why);
        if let Some(id) = self.revision.mutation_id() {
            let mut metadata = unknown(id);
            if let Some(mutation) = &mut metadata.mutation {
                mutation.outcome = "refused".into();
            }
            output.files = Some(Box::new(metadata));
            output.content = format!(
                "Native file mutation refused before effects; operation not retried.\n{}",
                output.content
            );
        }
        self.complete(output)
    }
}

fn unknown(id: &str) -> FileMetadata {
    FileMetadata {
        absence: None,
        revision_contract: Some(1),
        observations: Vec::new(),
        mutation: Some(magi_proto::tooling::FileMutation {
            id: id.into(),
            outcome: "unknown".into(),
            durability: None,
        }),
    }
}

pub(crate) fn presentation(mut output: crate::Output) -> crate::Output {
    if output.is_error
        && output.file_delivery.as_ref().is_some_and(|delivery| {
            delivery
                .0
                .metadata
                .mutation
                .as_ref()
                .is_some_and(|mutation| mutation.outcome == "committed")
        })
    {
        output.content = format!(
            "Native file mutation committed; error does not imply rollback.\n{}",
            output.content
        );
    }
    output
}

struct Completed {
    owner: Owner,
    cursor: u64,
    acknowledge: Arc<dyn Acknowledge>,
    metadata: FileMetadata,
    failed: bool,
    delivered: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
pub struct Delivery(Arc<Completed>);

impl Delivery {
    pub fn deliver(self, owner: &Owner, cursor: u64, failed: bool) {
        if self.0.owner.same(owner)
            && self.0.cursor == cursor
            && !self
                .0
                .delivered
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.0
                .acknowledge
                .delivered(&self.0.metadata, self.0.failed || failed);
        }
    }
}

impl PartialEq for Delivery {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Delivery {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("NativeFileDelivery")
    }
}

#[must_use]
pub fn valid(
    metadata: &FileMetadata,
    operation: FileOperation,
    id: Option<&str>,
    failed: bool,
) -> bool {
    if operation == FileOperation::Read && failed && *metadata == FileMetadata::default() {
        return true;
    }
    if metadata.revision_contract != Some(1)
        || metadata.observations.len() > 1
        || metadata.observations.iter().any(|observation| {
            observation.path_key.len() != 64
                || !observation
                    .path_key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || !magi_proto::tooling::files::receipt(&observation.revision)
                || observation.coverage != "snapshot"
        })
    {
        return false;
    }
    if let Some(absence) = &metadata.absence {
        return operation == FileOperation::Read
            && !failed
            && metadata.mutation.is_none()
            && metadata.observations.is_empty()
            && absence.version == 1
            && absence.path_key.len() == 64
            && absence
                .path_key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    }
    if operation == FileOperation::Read {
        return metadata.mutation.is_none() && (!failed || metadata.observations.is_empty());
    }
    let Some(mutation) = &metadata.mutation else {
        return false;
    };
    if Some(mutation.id.as_str()) != id {
        return false;
    }
    match mutation.outcome.as_str() {
        "committed" => matches!(
            mutation.durability.as_deref(),
            Some("synced" | "unconfirmed")
        ),
        "refused" | "unknown" => {
            failed && mutation.durability.is_none() && metadata.observations.is_empty()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
