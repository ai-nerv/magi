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
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};

/// One piece of work for the thread that owns the VM, and where to say it finished.
struct Job {
    session: Arc<Mutex<Session>>,
    kind: Work,
    done: oneshot::Sender<()>,
}

/// What a job is.
enum Work {
    /// Run a turn.
    Turn,
    /// Ask the model what the work ahead needs, then put each answer to the person.
    ///
    /// On this thread because it is a provider call against this session's context, and the
    /// adapter and client live here. It queues behind any turn already running, which is right:
    /// asking what a turn will need while one is in flight would describe the wrong work.
    Declare,
    /// Take on grants the parent of this session already holds.
    ///
    /// On this thread because the ledger is inside the `Ops` this thread owns. It queues like
    /// anything else, so a turn already running finishes under the permissions it started with
    /// rather than gaining new ones halfway through a tool call.
    TakeOn(Vec<magi_proto::permit::Grant>),
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
        // Nobody to ask, which is the same thing as nobody to gate: a worker with no UI behind
        // it refuses a permission and answers no question, and a tool that wanted one is told so.
        Self::gated(
            backend,
            None,
            std::sync::Arc::new(magi_tools::question::Unanswered),
            std::sync::Arc::new(magi_tools::holding::Screenless),
        )
    }

    /// The same, with somebody to ask when a tool wants to do something new.
    ///
    /// `None` is a worker nothing gates — the print-mode and test paths, where there is nobody
    /// to ask and refusing every action would make the tool set useless rather than safe.
    pub fn gated(
        backend: Backend,
        approver: Option<std::sync::Arc<dyn magi_tools::approve::Approver>>,
        asks: std::sync::Arc<dyn magi_tools::question::Asks>,
        holds: std::sync::Arc<dyn magi_tools::holding::Holds>,
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
            // The same sequence `magi tools` lists, from the one place that knows it. This ran
            // here and there and the two disagreed about the environment a process tool is
            // built with, so the listing described tools as they would never actually run.
            let (registry, _from_casper) = magi_lua::tool::assemble(
                std::rc::Rc::clone(&engine),
                std::sync::Arc::clone(&asks),
                std::sync::Arc::clone(&holds),
                &backend.environ,
            );
            // Gated when there is somebody to ask. The ledger starts with whatever the
            // configuration already granted, so a rule written down is not a question asked.
            let ops: std::rc::Rc<dyn magi_tools::Ops> = match (&approver, backend.confine) {
                // `confine` is honoured here too. The arm used to be `(Some(approver), _)`,
                // discarding it, so the setting applied only to sessions with nobody attached --
                // exactly the runs where a wall matters least, and never the ones where somebody
                // turned it on and watched it do nothing.
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
            // Lent to the VM so `magi.shell` has a seam to go through. The same `Ops` every
            // other tool acts through, so a Lua tool that runs a command is gated exactly as the
            // shell peer is.
            engine.borrow_mut().attach_ops(std::rc::Rc::clone(&ops));
            // Before the first turn, so the schema the model is given is the one the peers
            // actually implement rather than the one a config file claimed for them.
            registry.probe(&*ops);

            runtime.block_on(async {
                while let Some(job) = queue.recv().await {
                    match job.kind {
                        // A failed turn is already journalled as an error entry by `turn::run`;
                        // there is nothing further to report and nothing to abort the daemon for.
                        Work::Turn => {
                            let _ = turn::run(&job.session, &backend, &registry, &*ops).await;
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

    /// Run a turn for this session, and wait for it.
    ///
    /// Waiting is what makes a second prompt queue behind the first rather than interleave. The
    /// UI is not blocked by it: deltas are published as they arrive, from the worker.
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

/// Ask what the work ahead needs, then put each need through the ordinary prompt.
///
/// The answer is a proposal. Every need becomes an [`Ops::allow`] call — the same one a tool
/// makes — so the person sees the same prompt and the ledger is written by the same path. The
/// model gains nothing it did not have; what changes is that the questions arrive together, in
/// front of the work, described by the only party that knows the shape of it.
async fn declare(session: &Arc<Mutex<Session>>, backend: &Backend, ops: &dyn magi_tools::Ops) {
    // Said, not journalled. `Entry::Notice` is the UI's own device -- the protocol says the
    // daemon never authors one -- and a transcript replayed later should hold the conversation
    // rather than a proposal that was answered at the time. `Refused` is the existing path for
    // "the daemon has something to say that is not the conversation", and the UI already turns
    // one into a notice on screen.
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

    // Nothing to plan about. A provider handed a conversation with no messages does not answer
    // -- it does not refuse either, which is how this presented: a spinner and then nothing,
    // for as long as anybody was willing to watch.
    if context.messages.is_empty() {
        say(
            session,
            "Nothing to plan yet — say what you want done first, then ask again.".to_owned(),
        )
        .await;
        return;
    }
    // The question, last, so the conversation ends on something addressed to the model. Without
    // it the context ends on the model's own answer and the reply comes back empty.
    context
        .messages
        .push(magi_model::Message::user(crate::declaring::question(
            &backend.cwd,
        )));

    let wants = magi_proto::ask::Wants {
        schema: Some(crate::declaring::schema()),
        ..backend.wants.clone()
    };

    // Bounded. A mind that never answers must not hold the worker thread for the life of the
    // daemon, which is what a plain await here did.
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

    // Asked one at a time, through the ordinary gate, so each is answerable at any width and a
    // refusal is just a refusal. Blocking, so they queue rather than racing onto one screen.
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

    /// A worker with no backend cannot be built, so this checks the queue rather than a turn:
    /// a dropped worker must not leave a caller waiting forever.
    #[tokio::test]
    async fn a_dropped_worker_does_not_strand_its_caller() {
        let (jobs, queue) = mpsc::channel::<Job>(1);
        drop(queue);
        let worker = Worker { jobs };

        let dir = Scratch::new("magi-worker", "one");
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
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

        let dir = Scratch::new("magi-worker2", "one");
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
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
