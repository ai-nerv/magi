//! Per-prompt repeated-failure admission and source-ordered duplicate dispatch.

use magi_core::PendingCall;
use magi_tools::{Cancel, Ops, Output, Prepared, Registry};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ARGUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 1024;
const REPAIR: &str = "Repeated-call guard: not executed. This identical call failed twice without changed evidence. Change the approach or arguments; repeating it again will end this turn.";
const REPEATED: &str = "Repeated-call guard: stopped after the same failed call was requested again following repair guidance. No further unsent calls were executed.";
const CAPACITY: &str = "Repeated-call guard: stopped at the bounded failure-record limit. No further unsent calls were executed.";
const ARGUMENTS: &str = "Repeated-call guard: stopped because tool arguments exceed the 16 MiB admission limit. The call was not executed.";
const CANCELLED: &str = "cancelled before this tool ran";
type Key = [u8; 32];

#[derive(Default)]
pub(super) struct Progress {
    failures: BTreeMap<Key, Failure>,
    stopped: Option<&'static str>,
}

struct Failure {
    evidence: Option<Key>,
    streak: u8,
}

pub(super) struct Scheduled {
    key: Option<Key>,
    native: Option<String>,
    pending: Pending,
}

enum Pending {
    Ready(Prepared),
    Deferred,
    Refused(Output),
}

impl Progress {
    pub(super) fn stopped(&self) -> Option<&'static str> {
        self.stopped
    }

    fn admit(&mut self, key: Option<Key>) -> Result<(), &'static str> {
        if let Some(why) = self.stopped {
            return Err(why);
        }
        let Some(key) = key else {
            self.stopped = Some(ARGUMENTS);
            return Err(ARGUMENTS);
        };
        if let Some(failure) = self.failures.get_mut(&key) {
            if failure.streak >= 3 {
                self.stopped = Some(REPEATED);
                return Err(REPEATED);
            }
            if failure.streak == 2 {
                failure.streak = 3;
                return Err(REPAIR);
            }
        }
        Ok(())
    }

    fn completed(&mut self, key: Key, output: &Output) {
        if !output.is_error {
            self.failures.remove(&key);
            return;
        }
        let evidence = outcome(output);
        if let Some(failure) = self.failures.get_mut(&key) {
            if evidence.is_some() && failure.evidence != evidence {
                failure.evidence = evidence;
                failure.streak = 1;
            } else {
                failure.streak = failure.streak.saturating_add(1);
            }
        } else if self.failures.len() < MAX_RECORDS {
            self.failures.insert(
                key,
                Failure {
                    evidence,
                    streak: 1,
                },
            );
        } else {
            self.stopped = Some(CAPACITY);
        }
    }

    pub(super) fn prepare(
        &mut self,
        calls: &[PendingCall],
        registry: &Registry,
        ops: &dyn Ops,
        cancel: &dyn Cancel,
    ) -> Vec<Scheduled> {
        let mut seen = BTreeSet::new();
        calls
            .iter()
            .map(|call| {
                let key = fingerprint(&call.name, &call.arguments);
                let native = registry.get(&call.name).and_then(|tool| tool.file_scope());
                let pending = if cancel.is_cancelled() {
                    Pending::Refused(Output::error(CANCELLED))
                } else if native.is_some()
                    || key.is_some_and(|key| seen.contains(&key))
                    || seen.len() >= MAX_RECORDS
                {
                    Pending::Deferred
                } else {
                    if let Some(key) = key {
                        seen.insert(key);
                    }
                    match self.admit(key) {
                        Ok(()) => {
                            Pending::Ready(registry.prepare(&call.name, &call.arguments, ops))
                        }
                        Err(why) => Pending::Refused(Output::error(why)),
                    }
                };
                Scheduled {
                    key,
                    native,
                    pending,
                }
            })
            .collect()
    }

    pub(super) fn finish(
        &mut self,
        scheduled: Scheduled,
        call: &PendingCall,
        registry: &Registry,
        ops: &dyn Ops,
        cancel: &dyn Cancel,
        files: &crate::files::Files,
    ) -> Output {
        let key = match (&scheduled.native, scheduled.key) {
            (Some(scope), Some(key)) => {
                let arguments = magi_tools::repair::arguments(&call.arguments).ok();
                let path = arguments
                    .as_ref()
                    .and_then(|args| args.get("path"))
                    .and_then(serde_json::Value::as_str);
                let cwd = ops.cwd();
                let context = path.and_then(|path| {
                    files.progress(magi_tools::files::Subject {
                        supplier: scope,
                        cwd: cwd.to_str()?,
                        path,
                    })
                });
                hashed(&(key, context))
            }
            _ => scheduled.key,
        };
        let pending = match scheduled.pending {
            Pending::Deferred if !cancel.is_cancelled() => match self.admit(key) {
                Ok(()) => Pending::Ready(registry.prepare(&call.name, &call.arguments, ops)),
                Err(why) => Pending::Refused(Output::error(why)),
            },
            pending => pending,
        };
        match pending {
            Pending::Ready(prepared)
                if prepared.in_flight() || (!cancel.is_cancelled() && self.stopped.is_none()) =>
            {
                registry.finish_observed(prepared, ops, cancel, |output| {
                    if let Some(key) = key {
                        self.completed(key, output);
                    }
                })
            }
            Pending::Refused(output) => output,
            _ => Output::error(self.stopped.unwrap_or(CANCELLED)),
        }
    }
}

fn fingerprint(name: &str, arguments: &str) -> Option<Key> {
    if arguments.len() > MAX_ARGUMENT_BYTES {
        return None;
    }
    match magi_tools::repair::arguments(arguments) {
        Ok(mut value) => {
            value.sort_all_objects();
            hashed(&("json", name, value))
        }
        Err(_) => hashed(&("raw", name, arguments)),
    }
}

fn outcome(output: &Output) -> Option<Key> {
    let files = output
        .files
        .as_ref()
        .filter(|metadata| {
            metadata.absence.is_some()
                || !metadata.observations.is_empty()
                || metadata.mutation.is_some()
        })
        .map(|metadata| {
            (
                &metadata.absence,
                &metadata.observations,
                metadata
                    .mutation
                    .as_ref()
                    .map(|mutation| (&mutation.outcome, &mutation.durability)),
            )
        });
    if output.content.is_empty() && output.images.is_empty() && files.is_none() {
        return None;
    }
    hashed(&(&output.content, &output.images, files))
}

fn hashed(value: &impl Serialize) -> Option<Key> {
    let mut hash = HashWriter(Sha256::new());
    serde_json::to_writer(&mut hash, value).ok()?;
    Some(hash.0.finalize().into())
}

struct HashWriter(Sha256);

impl std::io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "progress/tests.rs"]
mod tests;
