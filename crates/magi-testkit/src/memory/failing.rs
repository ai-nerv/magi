use magi_ipc::family::{Faulted, Reply};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A legacy memory peer that negotiates successfully and fails every storage call.
pub struct Failing {
    socket: PathBuf,
    writes: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Failing {
    pub async fn start(socket: &Path) -> io::Result<Self> {
        let listener = tokio::net::UnixListener::bind(socket)?;
        let writes = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&writes);
        let task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept(), if clients.len() < 4 => {
                        let Ok((stream, _)) = accepted else { break; };
                        let seen = Arc::clone(&seen);
                        clients.spawn(async move {
                            let _ = tokio::time::timeout(std::time::Duration::from_secs(60), answer(stream, seen)).await;
                        });
                    }
                    _ = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Ok(Self {
            socket: socket.to_owned(),
            writes,
            task,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn failed_writes(&self) -> usize {
        self.writes.load(Ordering::Relaxed)
    }
}

impl Drop for Failing {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer(mut stream: tokio::net::UnixStream, writes: Arc<AtomicUsize>) -> io::Result<()> {
    loop {
        let size = stream.read_u32().await? as usize;
        if size > magi_ipc::family::MAX_FRAME_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).await?;
        let request: serde_json::Value = magi_ipc::Wire::read(&bytes)
            .map_err(|why| io::Error::new(io::ErrorKind::InvalidData, why))?;
        let call = request.get("call").and_then(serde_json::Value::as_str);
        let reply = if call == Some("verbs") {
            Reply::rows(vec![serde_json::json!({"name":"observe"})])
        } else {
            if call == Some("observe") {
                writes.fetch_add(1, Ordering::Relaxed);
            }
            let mut reply = Reply::refused("fixture storage unavailable");
            reply.fault = Some(Faulted::Failed);
            reply
        };
        let bytes = reply.encode(magi_ipc::Wire::of(&bytes));
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn startup_negotiates_but_observe_is_a_fatal_storage_failure() {
        let root = crate::Scratch::new("memory-failure", "wire");
        let memory = super::Failing::start(&root.join("s"))
            .await
            .expect("fixture");
        let mut family = magi_ipc::family::Family::dial(memory.socket())
            .await
            .expect("dial");
        let verbs = family.call("verbs", vec![]).await.expect("startup");
        assert_eq!(verbs, vec![serde_json::json!({"name":"observe"})]);
        assert!(matches!(
            family.call("observe", vec![]).await,
            Err(magi_ipc::family::Fault::Failed(_))
        ));
        assert_eq!(memory.failed_writes(), 1);
    }
}
