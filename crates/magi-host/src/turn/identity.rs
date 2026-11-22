use super::{Backend, Session};
use sha2::{Digest, Sha256};
use std::path::Path;

fn key(project: &Path, memory: Option<&Path>, transcript: &str) -> String {
    let mut hash = Sha256::new();
    for field in [
        b"magi-model-session-v1".as_slice(),
        project.as_os_str().as_encoded_bytes(),
        memory.map_or(b"unrecorded".as_slice(), |path| {
            path.as_os_str().as_encoded_bytes()
        }),
        transcript.as_bytes(),
    ] {
        sha2::Digest::update(&mut hash, (field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    format!("{:x}", hash.finalize())
}

pub(super) async fn for_turn(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    scribe: &crate::scribe::Held,
) -> Option<String> {
    if !backend.session_identity {
        return None;
    }
    let open = scribe.lock().await;
    if let Some(open) = open.as_ref() {
        return Some(key(&backend.cwd, Some(open.socket()), open.transcript()));
    }
    let held = session.lock().await;
    Some(key(
        &backend.cwd,
        None,
        backend
            .transcript
            .as_deref()
            .unwrap_or_else(|| held.id().as_str()),
    ))
}

#[cfg(test)]
mod tests;
