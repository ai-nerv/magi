//! The thread that owns the Lua VM. A protocol lives in a VM, and a VM is neither `Send` nor
//! `Sync`, so a turn cannot run on the connection task that asked for it: one thread, one VM, one
//! turn at a time. Two turns appending to one journal at once is a corrupt transcript.

use crate::session::Session;
use crate::turn::{self, Backend};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};

mod observing;

/// One piece of work for the thread that owns the VM, and where to say it finished.
struct Job {
    session: Arc<Mutex<Session>>,
    kind: Work,
    done: oneshot::Sender<Result<(), String>>,
}

/// Hard safety ceiling on consecutive auto-continued turns triggered by `after_turn` Lua hooks.
const MAX_AUTO_CONTINUES: usize = 25;

enum Work {
    Turn,
    /// Expand any inline `/command` tokens in a user prompt using the live Lua VM.
    DispatchCommand {
        prompt: String,
        aside: String,
        reply: oneshot::Sender<magi_lua::commands::Outcome>,
    },
    /// Ask the model what the work ahead needs, then put each answer to the person. On this thread
    /// because it is a provider call against this session's context, and it queues behind any turn.
    Declare,
    /// Take on grants the parent of this session already holds. On this thread because the ledger
    /// is inside the `Ops` it owns; it queues, so a running turn keeps the permissions it started with.
    TakeOn(Vec<magi_proto::permit::Grant>),
}

pub struct Worker {
    jobs: mpsc::Sender<Job>,
    reporting: Option<Backend>,
    granted: Arc<std::sync::Mutex<Vec<magi_proto::permit::Grant>>>,
}

impl Worker {
    pub(crate) fn backend(&self) -> Option<Backend> {
        let mut backend = self.reporting.clone()?;
        backend.grants = self.granted.lock().ok()?.clone();
        Some(backend)
    }

    pub(crate) fn held_grants(&self) -> Vec<magi_proto::permit::Grant> {
        self.granted
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub(crate) fn image_profile(&self) -> Option<(&str, Option<&magi_model::ImageResize>)> {
        self.reporting
            .as_ref()
            .map(|backend| (backend.model.as_str(), backend.image_resize.as_ref()))
    }

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
            std::sync::Arc::new(magi_tools::holding::Incurious),
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
        knows: std::sync::Arc<dyn magi_tools::holding::Answers>,
        scribe: crate::scribe::Held,
    ) -> Self {
        let reporting = backend.clone();
        let granted = Arc::new(std::sync::Mutex::new(backend.grants.clone()));
        let held_grants = Arc::clone(&granted);
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
            for (name, source) in backend.scripts.iter().chain(&backend.tools) {
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
            let (mut registry, _supplied) = magi_lua::tool::assemble(
                std::rc::Rc::clone(&engine),
                std::sync::Arc::clone(&asks),
                std::sync::Arc::clone(&holds),
                std::sync::Arc::clone(&knows),
                &backend.environ,
                &backend.tooling,
            );
            registry.set_filter(backend.tool_filter.clone());
            // Gated when there is somebody to ask; the ledger starts with what the config granted.
            let ops: std::rc::Rc<dyn magi_tools::Ops> = match (&approver, backend.confine) {
                // `confine` is honoured here too. The arm used to be `(Some(approver), _)`, which
                // discarded it, so the setting applied only to sessions with nobody attached.
                (Some(approver), confine) => std::rc::Rc::new(
                    magi_tools::ops::Real::gated(
                        backend.cwd.clone(),
                        magi_tools::permit::Ledger::with_profile(
                            backend.grants.clone(),
                            backend.profile_grants.clone(),
                        ),
                        std::sync::Arc::clone(approver),
                    )
                    .confining(confine)
                    .isolating(backend.isolate)
                    .observing(observing::authorization),
                ),
                (None, true) => std::rc::Rc::new(
                    magi_tools::ops::Real::confined(backend.cwd.clone())
                        .isolating(backend.isolate)
                        .observing(observing::authorization),
                ),
                (None, false) => std::rc::Rc::new(
                    magi_tools::ops::Real::new(backend.cwd.clone())
                        .isolating(backend.isolate)
                        .observing(observing::authorization),
                ),
            };
            // Lent to the VM so `magi.shell` goes through the same `Ops` every other tool acts through.
            engine.borrow_mut().attach_ops(std::rc::Rc::clone(&ops));
            // Before the first turn, so the schema the model is given is the one the peers implement.
            registry.probe(&*ops);

            runtime.block_on(async {
                let mut announced = None;
                while let Some(job) = queue.recv().await {
                    let (id, resumed) = {
                        let held = job.session.lock().await;
                        (held.id().clone(), !held.entries().is_empty())
                    };
                    let socket = scribe
                        .lock()
                        .await
                        .as_ref()
                        .map(|open| open.socket().to_owned());
                    engine
                        .borrow_mut()
                        .bind_session(id.as_str(), socket.as_deref());
                    if announced.as_ref() != Some(&id) {
                        registry.saw(&magi_tools::Event::Session {
                            id: id.as_str(),
                            resumed,
                        });
                        announced = Some(id);
                    }
                    let outcome = crate::catching::caught(async {
                        match job.kind {
                            Work::DispatchCommand {
                                prompt,
                                aside,
                                reply,
                            } => {
                                let outcome = magi_lua::commands::dispatch(
                                    &mut engine.borrow_mut(),
                                    &magi_lua::commands::Request {
                                        prompt: &prompt,
                                        aside: &aside,
                                        profile: backend.active_profile.as_deref(),
                                        model: Some(backend.model.as_str()),
                                    },
                                );
                                let _ = reply.send(outcome);
                                Ok(())
                            }
                            Work::Turn => {
                                run_turn_with_hooks(
                                    &job.session,
                                    &backend,
                                    &engine,
                                    &registry,
                                    &*ops,
                                    &scribe,
                                )
                                .await
                            }
                            Work::TakeOn(grants) => {
                                ops.take_on(handed(&*ops, &grants));
                                Ok(())
                            }
                            Work::Declare => {
                                let cancel = job.session.lock().await.cancel();
                                tokio::select! {
                                    biased;
                                    () = cancel.requested() => {},
                                    () = declare(&job.session, &backend, &*ops) => {},
                                }
                                Ok(())
                            }
                        }
                    })
                    .await
                    .unwrap_or_else(|why| {
                        Err(format!("this turn stopped on an internal error: {why}"))
                    });
                    if let Ok(mut granted) = held_grants.lock() {
                        *granted = ops.held();
                    }
                    let _ = job.done.send(outcome);
                }
            });
        });
        Self {
            jobs,
            reporting: Some(reporting),
            granted,
        }
    }

    /// Run a turn on this worker and wait for completion.
    pub async fn run(&self, session: Arc<Mutex<Session>>) -> Result<(), String> {
        let Some(reporting) = &self.reporting else {
            return self.queue(session, Work::Turn).await;
        };
        let start = session.lock().await.entries().len();
        let prepared = crate::reporting::prepare(&session, reporting).await;
        let outcome = match &prepared {
            Ok(_) => self.queue(Arc::clone(&session), Work::Turn).await,
            Err(why) => Err(why.clone()),
        };
        let reporting = crate::reporting::finish(
            &session,
            reporting,
            start,
            prepared.as_deref().unwrap_or_default(),
            &outcome,
        )
        .await;
        match (outcome, reporting) {
            (Err(why), Err(report)) => Err(format!(
                "{why}; final report could not be delivered: {report}"
            )),
            (Err(why), _) => Err(why),
            (Ok(()), result) => result,
        }
    }

    /// Expand any inline `/command` tokens in a user prompt using the worker's live Lua VM.
    pub async fn dispatch_command(
        &self,
        session: Arc<Mutex<Session>>,
        prompt: String,
        aside: String,
    ) -> Result<magi_lua::commands::Outcome, String> {
        let (reply, answer) = oneshot::channel();
        self.queue(
            session,
            Work::DispatchCommand {
                prompt,
                aside,
                reply,
            },
        )
        .await?;
        answer
            .await
            .map_err(|_| "session worker stopped before answering inline command".to_owned())
    }

    /// Ask the model what the work ahead needs, and put each answer to the person.
    pub async fn declare(&self, session: Arc<Mutex<Session>>) -> Result<(), String> {
        self.queue(session, Work::Declare).await
    }

    /// Take on grants this session's parent holds.
    pub async fn take_on(
        &self,
        session: Arc<Mutex<Session>>,
        grants: Vec<magi_proto::permit::Grant>,
    ) -> Result<(), String> {
        self.queue(session, Work::TakeOn(grants)).await
    }

    async fn queue(&self, session: Arc<Mutex<Session>>, kind: Work) -> Result<(), String> {
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
            return Err("session worker stopped before accepting the request".into());
        }
        finished
            .await
            .map_err(|_| "session worker stopped before finishing the request".to_owned())?
    }
}

/// Run one turn with `before_turn` / `after_turn` Lua hooks, supporting bounded autonomous loops
/// (e.g. an inline `/goal` command whose `after_turn` hook returns `{ continue = "..." }`).
async fn run_turn_with_hooks(
    session: &Arc<Mutex<Session>>,
    backend: &Backend,
    engine: &std::rc::Rc<std::cell::RefCell<magi_lua::Engine>>,
    registry: &magi_tools::Registry,
    ops: &dyn magi_tools::Ops,
    scribe: &crate::scribe::Held,
) -> Result<(), String> {
    let emit_notices = |session: &Arc<Mutex<Session>>, notices: Vec<String>| {
        let session = Arc::clone(session);
        async move {
            if notices.is_empty() {
                return;
            }
            let held = session.lock().await;
            let cursor = held.cursor();
            let publisher = held.publisher();
            for text in notices {
                let _ = publisher.send(magi_proto::HarnessEvent::Noticed { cursor, text });
            }
        }
    };

    for iteration in 0..MAX_AUTO_CONTINUES {
        let latest_prompt = {
            let held = session.lock().await;
            held.entries().iter().rev().find_map(|e| match e {
                magi_proto::Entry::User { text, .. } | magi_proto::Entry::Workflow { text, .. } => {
                    Some(text.clone())
                }
                _ => None,
            })
        };
        let before_out = magi_lua::hooks::before_turn(
            &mut engine.borrow_mut(),
            &magi_lua::hooks::TurnBefore {
                model: &backend.model,
                profile: backend.active_profile.as_deref(),
                iteration,
                prompt: latest_prompt.as_deref(),
            },
        );
        emit_notices(session, before_out.notices).await;

        let mut turn_backend = backend.clone();
        if !before_out.system.is_empty() {
            let mut parts: Vec<String> = turn_backend.system.into_iter().collect();
            parts.extend(before_out.system);
            turn_backend.system = Some(parts.join("\n\n"));
        }

        // What balthasar wants done between turns runs beside the next one.
        let over = crate::helping::between(
            Arc::clone(session),
            turn_backend.clone(),
            Arc::clone(scribe),
        )
        .await;
        let outcome = turn::run(session, &turn_backend, registry, ops, scribe)
            .await
            .map_err(|why| why.to_string());
        let (cancelled, last_text) = {
            let held = session.lock().await;
            let cancelled = held.cancel().is_requested();
            let text = held
                .entries()
                .iter()
                .rev()
                .find_map(|e| match e {
                    magi_proto::Entry::Assistant { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            (cancelled, text)
        };
        session.lock().await.rest();
        let _ = over.send(());

        let after_out = magi_lua::hooks::after_turn(
            &mut engine.borrow_mut(),
            &magi_lua::hooks::TurnAfter {
                model: &backend.model,
                profile: backend.active_profile.as_deref(),
                iteration,
                text: &last_text,
                ok: outcome.is_ok(),
                cancelled,
            },
        );
        emit_notices(session, after_out.notices).await;

        if outcome.is_err() || cancelled {
            return outcome;
        }
        let Some((next_prompt, next_aside)) = after_out.continue_prompt else {
            return Ok(());
        };
        if iteration + 1 >= MAX_AUTO_CONTINUES {
            emit_notices(
                session,
                vec![format!(
                    "Auto-continue loop stopped at safety limit ({MAX_AUTO_CONTINUES} turns)."
                )],
            )
            .await;
            return Ok(());
        }
        // Commit the follow-up turn prompt and loop.
        let mut held = session.lock().await;
        if held.cancel().is_requested() {
            return Ok(());
        }
        let id = magi_proto::MessageId::new(format!("u{}", held.cursor().next().0));
        held.commit(magi_proto::Entry::User {
            id,
            text: next_prompt,
            aside: next_aside,
            images: Vec::new(),
        })
        .map_err(|why| why.to_string())?;
    }
    Ok(())
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
        crate::broker::value_using(
            (&backend.mind, &backend.environ),
            &backend.model,
            &context,
            &wants,
        ),
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

/// What a handover from an adopting session may add: only what this session already holds.
fn handed(
    ops: &dyn magi_tools::Ops,
    offered: &[magi_proto::permit::Grant],
) -> Vec<magi_proto::permit::Grant> {
    magi_proto::permit::narrow(&ops.held(), offered)
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
        let worker = Worker {
            jobs,
            reporting: None,
            granted: Default::default(),
        };

        let _dir = Scratch::new("magi-worker", "one");
        let session = Session::recorded(SessionId::new("s"), Vec::new());
        let session = Arc::new(Mutex::new(session));

        // Returns rather than hanging: the send fails and there is nothing to wait for.
        worker
            .run(Arc::clone(&session))
            .await
            .expect_err("stopped worker reports failure");
        assert!(session.lock().await.entries().is_empty());
    }

    #[tokio::test]
    async fn a_session_is_usable_after_a_worker_refuses() {
        let (jobs, queue) = mpsc::channel::<Job>(1);
        drop(queue);
        let worker = Worker {
            jobs,
            reporting: None,
            granted: Default::default(),
        };

        let _dir = Scratch::new("magi-worker2", "one");
        let session = Session::recorded(SessionId::new("s"), Vec::new());
        let session = Arc::new(Mutex::new(session));

        worker
            .run(Arc::clone(&session))
            .await
            .expect_err("stopped worker reports failure");
        session
            .lock()
            .await
            .commit(Entry::User {
                images: Vec::new(),
                id: magi_proto::MessageId::new("u1"),
                text: "still works".into(),
                aside: String::new(),
            })
            .expect("commit");
        assert_eq!(session.lock().await.entries().len(), 1);
    }
}

#[cfg(test)]
mod handing {
    use magi_proto::permit::{Grant, Scope};

    fn program(name: &str) -> Grant {
        Grant {
            verb: "run".into(),
            scope: Scope::Program {
                program: name.into(),
            },
        }
    }

    #[test]
    fn a_handover_never_adds_what_the_session_did_not_hold() {
        let dir = magi_model::scratch::Scratch::new("magi-handover", "one");
        let ops = magi_tools::ops::Real::gated(
            dir.to_path_buf(),
            magi_tools::permit::Ledger::with(vec![program("git")]),
            std::sync::Arc::new(magi_tools::approve::DenyAll),
        );
        let offered = [program("git"), program("rm")];
        assert_eq!(super::handed(&ops, &offered), vec![program("git")]);
    }
}
