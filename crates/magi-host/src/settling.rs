//! Session-owned background tasks and cancellation-safe settlement.

use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;

type Task = JoinHandle<Result<(), String>>;

#[derive(Default)]
struct State {
    closed: bool,
    tasks: Vec<Task>,
    failure: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct Tasks(Arc<Mutex<State>>, Option<tokio::runtime::Handle>);

impl Tasks {
    /// Tasks that run on `runtime`, whichever runtime spawns them: the session's own, not the
    /// worker's, which blocks for the length of a tool call and goes when the model changes.
    pub(crate) fn on(runtime: tokio::runtime::Handle) -> Self {
        Self(Arc::default(), Some(runtime))
    }

    /// An empty set of tasks on the same runtime as these.
    pub(crate) fn fresh(&self) -> Self {
        Self(Arc::default(), self.1.clone())
    }

    pub(crate) fn spawn(
        &self,
        future: impl Future<Output = Result<(), String>> + Send + 'static,
    ) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|_| "helper tracker is poisoned")?;
        if state.closed {
            return Err("session is changing; helper was not accepted".into());
        }
        let mut failure = state.failure.take();
        state.tasks.retain_mut(|task| {
            if !task.is_finished() {
                return true;
            }
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            match std::pin::Pin::new(task).poll(&mut cx) {
                std::task::Poll::Ready(outcome) => {
                    if let Err(why) = outcome
                        .map_err(|why| format!("old session helper stopped: {why}"))
                        .and_then(|outcome| outcome)
                    {
                        failure.get_or_insert(why);
                    }
                    false
                }
                std::task::Poll::Pending => true,
            }
        });
        state.failure = failure;
        state.tasks.push(match &self.1 {
            Some(runtime) => runtime.spawn(future),
            None => tokio::spawn(future),
        });
        Ok(())
    }

    pub(crate) fn pause(&self) -> Result<Paused, String> {
        let mut state = self.0.lock().map_err(|_| "helper tracker is poisoned")?;
        if state.closed {
            return Err("session helper admission is already closed".into());
        }
        state.closed = true;
        Ok(Paused {
            tasks: self.clone(),
            retired: false,
        })
    }
}

pub(crate) struct Paused {
    tasks: Tasks,
    retired: bool,
}

impl Paused {
    pub(crate) async fn drain(&self, patience: std::time::Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            let task = {
                let mut state = self
                    .tasks
                    .0
                    .lock()
                    .map_err(|_| "helper tracker is poisoned")?;
                if let Some(why) = state.failure.take() {
                    return Err(why);
                }
                state.tasks.pop()
            };
            let Some(task) = task else {
                return Ok(());
            };
            let mut borrowed = Borrowed {
                task: Some(task),
                owner: self.tasks.clone(),
            };
            let outcome =
                tokio::time::timeout_at(deadline, borrowed.task.as_mut().expect("borrowed helper"))
                    .await
                    .map_err(|_| "old session helpers have not settled; retry resume later")?;
            borrowed.task.take();
            outcome.map_err(|why| format!("old session helper stopped: {why}"))??;
        }
    }

    pub(crate) fn retire(mut self) {
        self.retired = true;
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        if !self.retired
            && let Ok(mut state) = self.tasks.0.lock()
        {
            state.closed = false;
        }
    }
}

struct Borrowed {
    task: Option<Task>,
    owner: Tasks,
}

impl Drop for Borrowed {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            match self.owner.0.lock() {
                Ok(mut state) => state.tasks.push(task),
                Err(_) => task.abort(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_helpers_reap_finished_handles_without_losing_a_failure() {
        let tasks = Tasks::default();
        for fail in [false, true, false] {
            tasks
                .spawn(async move {
                    if fail {
                        Err("stored failure".into())
                    } else {
                        Ok(())
                    }
                })
                .expect("spawn");
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if tasks
                        .0
                        .lock()
                        .expect("tracker")
                        .tasks
                        .iter()
                        .all(Task::is_finished)
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("completed");
            assert_eq!(tasks.0.lock().expect("tracker").tasks.len(), 1);
        }
        let paused = tasks.pause().expect("pause");
        assert_eq!(
            paused
                .drain(std::time::Duration::from_secs(1))
                .await
                .expect_err("failure retained"),
            "stored failure"
        );
        paused
            .drain(std::time::Duration::from_secs(1))
            .await
            .expect("remaining helper");
    }

    #[tokio::test]
    async fn timeout_keeps_the_live_handle_and_failed_resume_reopens_admission() {
        let tasks = Tasks::default();
        let (send, receive) = tokio::sync::oneshot::channel();
        tasks
            .spawn(async move { receive.await.map_err(|why| why.to_string()) })
            .expect("spawn");
        let paused = tasks.pause().expect("pause");
        assert!(tasks.spawn(async { Ok(()) }).is_err());
        assert!(paused.drain(std::time::Duration::ZERO).await.is_err());
        assert_eq!(tasks.0.lock().expect("tracker").tasks.len(), 1);
        drop(paused);
        tasks.spawn(async { Ok(()) }).expect("admission reopened");
        send.send(()).expect("release live helper");
        tasks
            .pause()
            .expect("pause again")
            .drain(std::time::Duration::from_secs(1))
            .await
            .expect("both helpers drained");
        assert!(tasks.0.lock().expect("tracker").tasks.is_empty());
    }

    #[tokio::test]
    async fn cancelling_the_drain_does_not_detach_the_borrowed_helper() {
        let tasks = Tasks::default();
        let (send, receive) = tokio::sync::oneshot::channel();
        tasks
            .spawn(async move { receive.await.map_err(|why| why.to_string()) })
            .expect("spawn");
        let paused = tasks.pause().expect("pause");
        {
            let mut waiting = Box::pin(paused.drain(std::time::Duration::from_secs(30)));
            std::future::poll_fn(|cx| {
                assert!(waiting.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            assert!(tasks.0.lock().expect("tracker").tasks.is_empty());
        }
        assert_eq!(tasks.0.lock().expect("tracker").tasks.len(), 1);
        send.send(()).expect("helper still alive");
        paused
            .drain(std::time::Duration::from_secs(1))
            .await
            .expect("drained");
        paused.retire();
        assert!(tasks.spawn(async { Ok(()) }).is_err());
        assert!(tasks.pause().is_err());
    }

    #[tokio::test]
    async fn helper_failure_is_reported_without_discarding_other_handles() {
        let tasks = Tasks::default();
        let (send, receive) = tokio::sync::oneshot::channel();
        tasks
            .spawn(async move { receive.await.map_err(|why| why.to_string()) })
            .expect("spawn");
        tasks
            .spawn(async { Err("completion refused".into()) })
            .expect("spawn failed helper");
        let paused = tasks.pause().expect("pause");
        assert_eq!(
            paused
                .drain(std::time::Duration::from_secs(1))
                .await
                .expect_err("failed helper"),
            "completion refused"
        );
        assert_eq!(tasks.0.lock().expect("tracker").tasks.len(), 1);
        send.send(()).expect("release");
        paused
            .drain(std::time::Duration::from_secs(1))
            .await
            .expect("remaining helper");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_helper_runs_while_the_thread_that_started_it_is_blocked() {
        let tasks = Tasks::on(tokio::runtime::Handle::current());
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&ran);
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a worker runtime");
            runtime.block_on(async move {
                tasks
                    .spawn(async move {
                        seen.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    })
                    .expect("accepted");
                // A tool call: the worker's runtime polls nothing for its length.
                std::thread::sleep(std::time::Duration::from_millis(800));
            });
        });
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert!(
            ran.load(std::sync::atomic::Ordering::SeqCst),
            "the helper waited for the worker's tool call"
        );
        worker.join().expect("worker");
    }
}
