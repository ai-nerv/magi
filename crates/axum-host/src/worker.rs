//! The thread that owns the Lua VM.
//!
//! A protocol lives in a VM, and a VM is neither `Send` nor `Sync` — so a turn cannot run on
//! the connection task that asked for it. It runs here instead: one thread, one VM, one turn at
//! a time, fed by a channel.
//!
//! Serialising turns is not a limitation being worked around. A session has one conversation,
//! and two turns appending to one journal at once is a corrupt transcript however the VM
//! behaves.

use crate::session::Session;
use crate::turn::{self, Backend};
use axum_provider::client::Client;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};

/// One turn to run, and where to say it finished.
struct Job {
    session: Arc<Mutex<Session>>,
    done: oneshot::Sender<()>,
}

/// A handle to the thread running turns.
pub struct Worker {
    jobs: mpsc::Sender<Job>,
}

impl Worker {
    /// Start a worker owning `backend`.
    ///
    /// The thread lives as long as the daemon. It is not pooled: there is one VM because there
    /// is one description of each protocol, and a second would be a second copy to keep in step.
    #[must_use]
    pub fn start(backend: Backend) -> Self {
        Self::gated(backend, None)
    }

    /// The same, with somebody to ask when a tool wants to do something new.
    ///
    /// `None` is a worker nothing gates — the print-mode and test paths, where there is nobody
    /// to ask and refusing every action would make the tool set useless rather than safe.
    pub fn gated(
        backend: Backend,
        approver: Option<std::sync::Arc<dyn axum_tools::approve::Approver>>,
    ) -> Self {
        let (jobs, mut queue) = mpsc::channel::<Job>(32);
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            // The VM is built here, not handed over: it cannot cross a thread boundary, and
            // building it where it lives is also where a broken protocol description should
            // surface.
            // One VM for the thread: the protocol reads it and every Lua tool runs in it. Built
            // here because it cannot cross a thread boundary, and this is also where a broken
            // description should surface.
            let mut engine = axum_lua::Engine::new();
            engine.install_stubs(&backend.stubs);
            let mut broken = None;
            for (name, source) in &backend.apis {
                if let Err(why) = engine.run(source, name) {
                    broken = Some(why.to_string());
                }
            }
            for (name, source) in &backend.tools {
                if let Err(why) = engine.run(source, name) {
                    broken = Some(why.to_string());
                }
            }
            if let Some(why) = broken {
                eprintln!("axum host: {why}");
                return;
            }

            let engine = std::rc::Rc::new(std::cell::RefCell::new(engine));
            let mut registry = axum_tools::Registry::new();
            axum_tools::builtin::install(&mut registry);
            axum_lua::tool::install(std::rc::Rc::clone(&engine), &mut registry);
            // Gated when there is somebody to ask. The ledger starts with whatever the
            // configuration already granted, so a rule written down is not a question asked.
            let ops = match (&approver, backend.confine) {
                (Some(approver), _) => axum_tools::ops::Real::gated(
                    backend.cwd.clone(),
                    axum_tools::permit::Ledger::with(backend.grants.clone()),
                    std::sync::Arc::clone(approver),
                ),
                (None, true) => axum_tools::ops::Real::confined(backend.cwd.clone()),
                (None, false) => axum_tools::ops::Real::new(backend.cwd.clone()),
            };
            // Before the first turn, so the schema the model is given is the one the peers
            // actually implement rather than the one a config file claimed for them.
            registry.probe(&ops);

            let built = axum_lua::adapter::LuaAdapter::from_shared(
                std::rc::Rc::clone(&engine),
                backend.model.api.as_str(),
            );
            let adapter = match built {
                Ok(adapter) => adapter,
                Err(why) => {
                    eprintln!("axum host: {why}");
                    return;
                }
            };
            let client = Client::new();
            runtime.block_on(async {
                while let Some(job) = queue.recv().await {
                    // A failed turn is already journalled as an error entry by `turn::run`;
                    // there is nothing further to report and nothing to abort the daemon for.
                    let _ =
                        turn::run(&job.session, &backend, &adapter, &client, &registry, &ops).await;
                    let _ = job.done.send(());
                }
            });
        });
        Self { jobs }
    }

    /// Run a turn for this session, and wait for it.
    ///
    /// Waiting is what makes a second prompt queue behind the first rather than interleave. The
    /// UI is not blocked by it: deltas are published as they arrive, from the worker.
    pub async fn run(&self, session: Arc<Mutex<Session>>) {
        let (done, finished) = oneshot::channel();
        if self.jobs.send(Job { session, done }).await.is_err() {
            return;
        }
        let _ = finished.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_proto::{Entry, SessionId};

    /// A worker with no backend cannot be built, so this checks the queue rather than a turn:
    /// a dropped worker must not leave a caller waiting forever.
    #[tokio::test]
    async fn a_dropped_worker_does_not_strand_its_caller() {
        let (jobs, queue) = mpsc::channel::<Job>(1);
        drop(queue);
        let worker = Worker { jobs };

        let dir = std::env::temp_dir().join(format!("axum-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
        let session = Arc::new(Mutex::new(session));

        // Returns rather than hanging: the send fails and there is nothing to wait for.
        worker.run(Arc::clone(&session)).await;
        assert!(session.lock().await.entries().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_session_is_usable_after_a_worker_refuses() {
        let (jobs, queue) = mpsc::channel::<Job>(1);
        drop(queue);
        let worker = Worker { jobs };

        let dir = std::env::temp_dir().join(format!("axum-worker2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
        let session = Arc::new(Mutex::new(session));

        worker.run(Arc::clone(&session)).await;
        session
            .lock()
            .await
            .commit(Entry::User {
                id: axum_proto::MessageId::new("u1"),
                text: "still works".into(),
            })
            .expect("commit");
        assert_eq!(session.lock().await.entries().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
