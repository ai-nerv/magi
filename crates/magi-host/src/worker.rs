//! The thread that owns the Lua VM. A protocol lives in a VM, and a VM is neither `Send` nor
//! `Sync`, so a turn cannot run on the connection task that asked for it: one thread, one VM, one
//! turn at a time. Two turns appending to one journal at once is a corrupt transcript.

use crate::session::Session;
use crate::turn::{self, Backend};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};

/// One piece of work for the thread that owns the VM, and where to say it finished.
struct Job {
    session: Arc<Mutex<Session>>,
    kind: Work,
    done: oneshot::Sender<()>,
}

enum Work {
    Turn,
    /// Ask the model what the work ahead needs, then put each answer to the person. On this thread
    /// because it is a provider call against this session's context, and it queues behind any turn.
    Declare,
    /// Take on grants the parent of this session already holds. On this thread because the ledger
    /// is inside the `Ops` it owns; it queues, so a running turn keeps the permissions it started with.
    TakeOn(Vec<magi_proto::permit::Grant>),
}

pub struct Worker {
    jobs: mpsc::Sender<Job>,
}

impl Worker {
    /// Start a worker owning `backend`. The thread lives as long as the daemon, and is not pooled:
    /// there is one VM because there is one description of each protocol.
    #[must_use]
    pub fn start(backend: Backend) -> Self {
        // Nobody to ask is the same as nobody to gate: this worker refuses a permission, answers no
        // question, and tells a tool that wanted one so.
        Self::gated(
            backend,
            None,
            std::sync::Arc::new(magi_tools::question::Unanswered),
            std::sync::Arc::new(magi_tools::holding::Screenless),
            // And no memory layer: the worker a test or a one-shot builds.
            std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        )
    }

    /// The same, with somebody to ask when a tool wants to do something new. `None` is a worker
    /// nothing gates — the print-mode and test paths.
    pub fn gated(
        backend: Backend,
        approver: Option<std::sync::Arc<dyn magi_tools::approve::Approver>>,
        asks: std::sync::Arc<dyn magi_tools::question::Asks>,
        holds: std::sync::Arc<dyn magi_tools::holding::Holds>,
        scribe: crate::scribe::Held,
    ) -> Self {
        let (jobs, mut queue) = mpsc::channel::<Job>(32);
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            // One VM for the thread: the protocol reads it and every Lua tool runs in it. Built
            // here because it cannot cross a thread boundary.
            let mut engine = magi_lua::Engine::new();
            engine.install_clients(&backend.clients);
            let mut broken = None;
            for (name, source) in &backend.tools {
                if let Err(why) = engine.run(source, name) {
                    broken = Some(why.to_string());
                }
            }
            if let Some(why) = broken {
                eprintln!("magi host: {why}");
                return;
            }

            let engine = std::rc::Rc::new(std::cell::RefCell::new(engine));
            // The same sequence `magi tools` lists, from the one place that knows it.
            let (registry, _from_casper) = magi_lua::tool::assemble(
                std::rc::Rc::clone(&engine),
                std::sync::Arc::clone(&asks),
                std::sync::Arc::clone(&holds),
                &backend.environ,
                &backend.tooling,
            );
            // Gated when there is somebody to ask; the ledger starts with what the config granted.
            let ops: std::rc::Rc<dyn magi_tools::Ops> = match (&approver, backend.confine) {
                // `confine` is honoured here too. The arm used to be `(Some(approver), _)`, which
                // discarded it, so the setting applied only to sessions with nobody attached.
                (Some(approver), confine) => std::rc::Rc::new(
                    magi_tools::ops::Real::gated(
                        backend.cwd.clone(),
                        magi_tools::permit::Ledger::with(backend.grants.clone()),
                        std::sync::Arc::clone(approver),
                    )
                    .confining(confine),
                ),
                (None, true) => {
                    std::rc::Rc::new(magi_tools::ops::Real::confined(backend.cwd.clone()))
                }
                (None, false) => std::rc::Rc::new(magi_tools::ops::Real::new(backend.cwd.clone())),
            };
            // Lent to the VM so `magi.shell` goes through the same `Ops` every other tool acts through.
            engine.borrow_mut().attach_ops(std::rc::Rc::clone(&ops));
            // Before the first turn, so the schema the model is given is the one the peers implement.
            registry.probe(&*ops);

            runtime.block_on(async {
                // Said once, when the worker first has a session in hand: the earliest point where
                // both the session and the registry the watchers live in exist. Resumed is read off
                // the session — one that already has entries was carried in from a journal.
                let mut announced = false;
                while let Some(job) = queue.recv().await {
                    if !announced {
                        announced = true;
                        let held = job.session.lock().await;
                        registry.saw(&magi_tools::Event::Session {
                            id: held.id().as_str(),
                            resumed: !held.entries().is_empty(),
                        });
                    }
                    match job.kind {
                        // A failed turn is already journalled as an error entry by `turn::run`.
                        Work::Turn => {
                            let _ =
                                turn::run(&job.session, &backend, &registry, &*ops, &scribe).await;
                        }
                        Work::TakeOn(grants) => ops.take_on(grants),
                        Work::Declare => {
                            declare(&job.session, &backend, &*ops).await;
                        }
                    }
                    let _ = job.done.send(());
                }
            });
        });
        Self { jobs }
    }

    /// Run a turn for this session, and wait for it. Waiting is what makes a second prompt queue
    /// behind the first; deltas are published from the worker, so the UI is not blocked by it.
    pub async fn run(&self, session: Arc<Mutex<Session>>) {
        self.queue(session, Work::Turn).await;
    }

    /// Ask the model what the work ahead needs, and put each answer to the person.
    pub async fn declare(&self, session: Arc<Mutex<Session>>) {
        self.queue(session, Work::Declare).await;
    }

    /// Take on grants this session's parent holds.
    pub async fn take_on(
        &self,
        session: Arc<Mutex<Session>>,
        grants: Vec<magi_proto::permit::Grant>,
    ) {
        self.queue(session, Work::TakeOn(grants)).await;
    }

    async fn queue(&self, session: Arc<Mutex<Session>>, kind: Work) {
        let (done, finished) = oneshot::channel();
        if self
            .jobs
            .send(Job {
                session,
                kind,
                done,
            })
            .await
            .is_err()
        {
            return;
        }
        let _ = finished.await;
    }
}

/// Ask what the work ahead needs, then put each need through the ordinary prompt. Every need
/// becomes an [`Ops::allow`] call, so the person sees the same prompt and the same ledger is written.
async fn declare(session: &Arc<Mutex<Session>>, backend: &Backend, ops: &dyn magi_tools::Ops) {
    // Said, not journalled: `Entry::Notice` is the UI's own device and the daemon never authors
    // one. `Refused` is the path for something the daemon says that is not the conversation.
    let say = |session: &Arc<Mutex<Session>>, message: String| {
        let session = Arc::clone(session);
        async move {
            let held = session.lock().await;
            let _ = held.publisher().send(magi_proto::HarnessEvent::Refused {
                cursor: held.cursor(),
                message,
            });
        }
    };

    let mut context = crate::context::of(&*session.lock().await);
    context.system.clone_from(&backend.system);

    // A provider handed a conversation with no messages does not answer, and does not refuse either.
    if context.messages.is_empty() {
        say(
            session,
            "Nothing to plan yet — say what you want done first, then ask again.".to_owned(),
        )
        .await;
        return;
    }
    // The question last, so the context does not end on the model's own answer and come back empty.
    context
        .messages
        .push(magi_model::Message::user(crate::declaring::question(
            &backend.cwd,
        )));

    let wants = magi_proto::ask::Wants {
        schema: Some(crate::declaring::schema()),
        ..backend.wants.clone()
    };

    // Bounded: a mind that never answers must not hold the worker thread for the life of the daemon.
    let asked = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        crate::broker::value(&backend.model, &context, &wants),
    );
    let answer = match asked.await {
        Err(_) => {
            say(
                session,
                "The model did not answer what it needs within two minutes.".to_owned(),
            )
            .await;
            return;
        }
        Ok(Ok(value)) => value,
        Ok(Err(why)) => {
            say(session, format!("Could not ask what this needs: {why}")).await;
            return;
        }
    };

    let needs = crate::declaring::read(&answer);
    if needs.is_empty() {
        say(session, "The model asked for no permissions.".to_owned()).await;
        return;
    }

    {
        let lines: Vec<String> = needs
            .iter()
            .map(|need| format!("{} {} — {}", need.verb, need.scope, need.why))
            .collect();
        say(session, format!("It says it needs: {}", lines.join("; "))).await;
    }

    // One at a time, through the ordinary gate. Blocking, so they queue rather than race onto one screen.
    for need in needs {
        let Some(grant) = need.grant() else { continue };
        let action = match &grant.scope {
            magi_proto::permit::Scope::Program { program } => magi_proto::permit::Action::Run {
                command: program.clone(),
                program: program.clone(),
            },
            magi_proto::permit::Scope::Directory { path } => match need.verb.as_str() {
                "write" => magi_proto::permit::Action::Write { path: path.clone() },
                "reach" => magi_proto::permit::Action::Network { host: path.clone() },
                _ => magi_proto::permit::Action::Read { path: path.clone() },
            },
            _ => continue,
        };
        let _ = ops.allow("the work ahead", &action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::scratch::Scratch;
    use magi_proto::{Entry, SessionId};

    /// A dropped worker must not leave a caller waiting forever.
    #[tokio::test]
    async fn a_dropped_worker_does_not_strand_its_caller() {
        let (jobs, queue) = mpsc::channel::<Job>(1);
        drop(queue);
        let worker = Worker { jobs };

        let _dir = Scratch::new("magi-worker", "one");
        let session = Session::recorded(SessionId::new("s"), Vec::new());
        let session = Arc::new(Mutex::new(session));

        // Returns rather than hanging: the send fails and there is nothing to wait for.
        worker.run(Arc::clone(&session)).await;
        assert!(session.lock().await.entries().is_empty());
    }

    #[tokio::test]
    async fn a_session_is_usable_after_a_worker_refuses() {
        let (jobs, queue) = mpsc::channel::<Job>(1);
        drop(queue);
        let worker = Worker { jobs };

        let _dir = Scratch::new("magi-worker2", "one");
        let session = Session::recorded(SessionId::new("s"), Vec::new());
        let session = Arc::new(Mutex::new(session));

        worker.run(Arc::clone(&session)).await;
        session
            .lock()
            .await
            .commit(Entry::User {
                id: magi_proto::MessageId::new("u1"),
                text: "still works".into(),
                aside: String::new(),
            })
            .expect("commit");
        assert_eq!(session.lock().await.entries().len(), 1);
    }
}
