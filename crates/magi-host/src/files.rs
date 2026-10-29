use magi_proto::tooling::{
    FileCapability, FileMetadata, FileObservation, FileOperation, FileRevision,
};
use magi_tools::files::{Acknowledge, Grant, Owner, Revisions, Subject};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

const ENTRIES: usize = 512;
const PATH_BYTES: usize = 4096;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    supplier: String,
    cwd: String,
    path: String,
}

struct Entry {
    generation: u64,
    pending: bool,
    needs_read: bool,
    observation: Option<FileObservation>,
}

#[derive(Default)]
struct State {
    retired: bool,
    nonce: Option<String>,
    next: u64,
    entries: BTreeMap<Key, Entry>,
}

#[derive(Clone, Default)]
pub(crate) struct Files {
    owner: Owner,
    state: Arc<Mutex<State>>,
}

impl Files {
    pub(crate) fn progress(&self, subject: Subject<'_>) -> Option<(bool, Option<FileObservation>)> {
        if subject.supplier.len() != 64
            || subject.cwd.len() > PATH_BYTES
            || subject.path.len() > PATH_BYTES
        {
            return None;
        }
        let state = self.state.lock().ok()?;
        if state.retired {
            return None;
        }
        let key = Key {
            supplier: subject.supplier.into(),
            cwd: subject.cwd.into(),
            path: subject.path.into(),
        };
        Some(state.entries.get(&key).map_or((false, None), |entry| {
            (entry.needs_read, entry.observation.clone())
        }))
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        self.owner.same(&other.owner)
    }
    pub(crate) fn call(&self, cursor: u64) -> Call {
        Call {
            files: self.clone(),
            cursor,
        }
    }
    pub(crate) fn delivered(
        &self,
        delivery: magi_tools::files::Delivery,
        cursor: u64,
        failed: bool,
    ) {
        delivery.deliver(&self.owner, cursor, failed);
    }
    pub(crate) fn retire(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.retired = true;
            state.entries.clear();
        }
    }
}

pub(crate) struct Call {
    files: Files,
    cursor: u64,
}

impl Revisions for Call {
    fn admit(&self, subject: Subject<'_>, capability: &FileCapability) -> Result<Grant, String> {
        let (revision, acknowledge) = self.reserve(subject, capability)?;
        Ok(Grant::new(
            revision,
            capability.operation,
            self.files.owner.clone(),
            self.cursor,
            Arc::new(acknowledge),
        ))
    }
}

impl Call {
    fn reserve(
        &self,
        subject: Subject<'_>,
        capability: &FileCapability,
    ) -> Result<(FileRevision, Receipt), String> {
        let bounded = |value: &str| {
            !value.is_empty() && value.len() <= PATH_BYTES && !value.as_bytes().contains(&0)
        };
        if capability.version != 1
            || subject.supplier.len() != 64
            || !subject
                .supplier
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !bounded(subject.cwd)
            || !bounded(subject.path)
            || !std::path::Path::new(subject.cwd).is_absolute()
        {
            return Err("native file observation subject is incomplete or unsupported".into());
        }
        let key = Key {
            supplier: subject.supplier.into(),
            cwd: subject.cwd.into(),
            path: subject.path.into(),
        };
        let mut state = self
            .files
            .state
            .lock()
            .map_err(|_| "native file observations are unavailable")?;
        if state.retired {
            return Err(
                "native file delivery context has retired; reread in the current session".into(),
            );
        }
        let expected = state
            .entries
            .get(&key)
            .and_then(|entry| entry.observation.as_ref())
            .map(|observation| observation.revision.clone());
        let needs_read = state
            .entries
            .get(&key)
            .is_some_and(|entry| entry.needs_read);
        if capability.operation == FileOperation::Write && needs_read {
            return Err(
                "native file mutation requires a delivered reread after its prior attempt".into(),
            );
        }
        if capability.operation == FileOperation::Edit && expected.is_none() {
            return Err(
                "native edit requires a delivered observation in this agent session; reread first"
                    .into(),
            );
        }
        let next = state
            .next
            .checked_add(1)
            .ok_or("native logical mutation identities are exhausted")?;
        if state.nonce.is_none() {
            state.nonce = Some(
                magi_ipc::random_hex(16)
                    .map_err(|_| "native logical identities are unavailable")?,
            );
        }
        let id = format!(
            "{}-{next}",
            state.nonce.as_deref().ok_or("native identity is missing")?
        );
        let mut revision = FileRevision::native(
            capability,
            if capability.operation == FileOperation::Read {
                None
            } else {
                Some((expected.as_deref().unwrap_or("absent"), &id))
            },
        )?;
        if needs_read && capability.operation == FileOperation::Read {
            revision = revision.recovery()?;
        }
        let recovering = revision.recovering();
        if !state.entries.contains_key(&key) && state.entries.len() >= ENTRIES {
            let oldest = state
                .entries
                .iter()
                .filter(|(_, entry)| !entry.needs_read)
                .min_by_key(|(_, entry)| entry.generation)
                .map(|(key, _)| key.clone())
                .ok_or("native revision map is full of unresolved mutations; reread an existing target first")?;
            state.entries.remove(&oldest);
        }
        state.next = next;
        state.entries.insert(
            key.clone(),
            Entry {
                generation: next,
                pending: true,
                needs_read: needs_read || capability.operation != FileOperation::Read,
                observation: None,
            },
        );
        Ok((
            revision,
            Receipt {
                state: Arc::downgrade(&self.files.state),
                key,
                generation: next,
                operation: capability.operation,
                mutation_id: (capability.operation != FileOperation::Read).then_some(id),
                recovering,
            },
        ))
    }
}

struct Receipt {
    state: Weak<Mutex<State>>,
    key: Key,
    generation: u64,
    operation: FileOperation,
    mutation_id: Option<String>,
    recovering: bool,
}

impl Acknowledge for Receipt {
    fn delivered(&self, metadata: &FileMetadata, failed: bool) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let Ok(mut state) = state.lock() else {
            return;
        };
        if state.retired {
            return;
        }
        let Some(entry) = state.entries.get_mut(&self.key) else {
            return;
        };
        if !entry.pending || entry.generation != self.generation {
            return;
        }
        entry.pending = false;
        if (metadata.absence.is_none() || self.recovering)
            && magi_tools::files::valid(
                metadata,
                self.operation,
                self.mutation_id.as_deref(),
                failed,
            )
        {
            entry.observation = metadata.observations.first().cloned();
            if entry.observation.is_some()
                || metadata.absence.is_some()
                || metadata
                    .mutation
                    .as_ref()
                    .is_some_and(|mutation| mutation.outcome == "refused")
            {
                entry.needs_read = false;
            }
        }
    }
}

#[cfg(test)]
mod tests;
