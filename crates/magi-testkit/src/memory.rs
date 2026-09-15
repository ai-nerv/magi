//! A memory layer of a test's own, so nothing a test writes reaches the store of whoever runs it.

use std::path::{Path, PathBuf};

/// A `balthasar serve` a test started, killed when dropped.
pub struct Serving {
    child: std::process::Child,
    socket: PathBuf,
}

impl Serving {
    /// Start one rooted at `dir` under `instance` and wait until it answers; `None` when there is
    /// no balthasar to start.
    ///
    /// # Panics
    /// When it started and never answered, which is a failure and not a skip.
    pub async fn start(dir: &Path, instance: &str) -> Option<Self> {
        let runtime = dir.join("r");
        let child = std::process::Command::new("balthasar")
            .arg("serve")
            .arg("--instance")
            .arg(instance)
            .arg("--scope")
            .arg("project")
            .current_dir(dir)
            .env("XDG_RUNTIME_DIR", &runtime)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        // Both of the role's directories: one not yet rebuilt binds only the older name.
        let sockets = ["memory", "balthasar"]
            .map(|role| runtime.join(role).join(format!("api@{instance}.sock")));
        let mut serving = Self {
            child,
            socket: sockets[0].clone(),
        };

        // Answering, not merely bound, on the durable clock: the first call opens the store.
        let patience = magi_ipc::family::DURABLE;
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            for socket in &sockets {
                let Ok(mut family) = magi_ipc::family::Family::dial(socket).await else {
                    continue;
                };
                let asked = vec![serde_json::Value::String(instance.to_owned())];
                if family.call_within("replay", asked, patience).await.is_ok() {
                    serving.socket.clone_from(socket);
                    return Some(serving);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        drop(serving);
        panic!("balthasar started and did not answer within {patience:?}");
    }

    /// The socket it answers on.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
