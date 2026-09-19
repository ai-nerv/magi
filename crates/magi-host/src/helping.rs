//! Small models run on somebody else's behalf. balthasar wants a summary or a tidy-up, a surface
//! wants to know whether something is safe; magi owns the providers, so magi runs them — with the
//! model `magi.helpers` names for the role — hands back what came, and says what it cost.

use crate::catalog::Backend;
use magi_proto::HarnessEvent;

mod attempt;
#[cfg(test)]
mod retrying;

pub use crate::catalog::Helpers;
pub use magi_proto::laying::Job;

pub use crate::catalog::Spend;

/// How long a job may take when neither it nor the configuration says.
const TIMEOUT_MS: u64 = 20_000;

/// How long an answer may be when the job does not say.
const MAX_TOKENS: u64 = 4_000;

/// What a helper said, who said it, and what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    pub model: String,
    pub usage: magi_proto::Usage,
}

#[derive(Debug, Default)]
pub(crate) struct Failure {
    pub(crate) message: String,
    pub(crate) model: String,
    pub(crate) usage: magi_proto::Usage,
}

/// A helper role set to this runs on the session's own model.
pub const MAIN: &str = "main";

/// The roles balthasar's jobs ask for, each of which is `memory`'s when nobody named it: one line
/// in a configuration keeps working, and a person who wants a stronger model for summaries than
/// for notes says so for that role alone.
const OF_MEMORY: &[&str] = &["summary", "notes", "curate"];

impl Helpers {
    /// The model to run `job` with, or `None` when nothing should.
    #[must_use]
    pub fn model_for(&self, job: &Job, main: &str) -> Option<String> {
        // `main` is the session's own model by name: a review is the main model's to make.
        if job.role == "main" && !main.is_empty() {
            return Some(main.to_owned());
        }
        let named = self.roles.get(&job.role).or_else(|| {
            OF_MEMORY
                .contains(&job.role.as_str())
                .then(|| self.roles.get("memory"))
                .flatten()
        });
        match named.map(String::as_str) {
            Some(MAIN) => Some(main.to_owned()).filter(|m| !m.is_empty()),
            Some(model) => Some(model.to_owned()),
            None => (job.fallback == "main" && !main.is_empty()).then(|| main.to_owned()),
        }
    }

    fn patience(&self, job: &Job) -> std::time::Duration {
        let configured = Some(self.timeout_ms).filter(|&ms| ms > 0);
        std::time::Duration::from_millis(job.timeout_ms.or(configured).unwrap_or(TIMEOUT_MS))
    }
}

/// Run one job and wait for its answer.
///
/// # Errors
/// Why there is no answer: no model for the role, a refusal, silence, or nothing said.
pub async fn run(job: &Job, backend: &Backend) -> Result<Answer, String> {
    run_accounted(job, backend)
        .await
        .map_err(|failure| failure.message)
}

pub(crate) async fn run_accounted(job: &Job, backend: &Backend) -> Result<Answer, Failure> {
    let model = backend
        .helpers
        .model_for(job, &backend.model)
        .ok_or_else(|| Failure {
            message: format!("no helper is configured for `{}`", job.role),
            ..Failure::default()
        })?;
    let context = magi_model::Context {
        system: instructed(job),
        messages: vec![magi_model::Message::user(job.input.clone())],
        tools: Vec::new(),
    };
    let wants = wants(job, backend, &model);
    let patience = backend.helpers.patience(job);

    let attempt = std::sync::Mutex::new(attempt::Attempt::default());
    let asked = tokio::time::timeout(
        patience,
        crate::broker::ask_through(
            &backend.mind,
            &model,
            &context,
            &wants,
            |delta| {
                attempt
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .apply(delta);
            },
            |_| {
                attempt
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .retry();
            },
        ),
    )
    .await;
    let attempt = attempt
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let usage = attempt.usage();
    let text = match asked {
        Err(_) => Err(format!("{model} did not answer within {patience:?}")),
        Ok(Err(trouble)) => Err(trouble.message),
        Ok(Ok(())) => attempt.text(job.schema.as_ref().is_some_and(|schema| !schema.is_null())),
    }
    .map_err(|message| Failure {
        message,
        model: model.clone(),
        usage,
    })?;
    Ok(Answer { text, model, usage })
}

/// What a job asks its model for. No reasoning: a helper is there to be quick and cheap, and one
/// left to reason spent a whole budget thinking and answered nothing.
fn wants(job: &Job, backend: &Backend, model: &str) -> magi_proto::ask::Wants {
    magi_proto::ask::Wants {
        thinking: Some(magi_model::ThinkingLevel::Off),
        max_tokens: Some(job.max_tokens.unwrap_or(MAX_TOKENS)),
        // Asked for in words rather than forced: a small model held to a schema from the first token
        // has nowhere to think but inside the strings, and wrote its reasoning into a note's title.
        schema: None,
        // The routing chosen for the session's model means nothing to another one.
        provider: (model == backend.model)
            .then(|| backend.wants.provider.clone())
            .flatten(),
    }
}

/// A job's instruction, with the shape its answer must take said in words when it has one.
fn instructed(job: &Job) -> Option<String> {
    let shape = job.schema.as_ref().filter(|schema| !schema.is_null());
    let mut said = job.instruction.clone();
    if let Some(shape) = shape {
        said.push_str(&format!(
            "\n\nAnswer with one JSON value matching this JSON Schema, and nothing else:\n{shape}"
        ));
    }
    Some(said).filter(|s| !s.trim().is_empty())
}

/// Run each job, tell balthasar how each went, and tell whoever watches what each cost. One at a
/// time: they share the prompt's budget, and a job that would pass it is not started.
pub async fn work(
    jobs: &[Job],
    backend: &Backend,
    scribe: &crate::scribe::Held,
    events: &tokio::sync::broadcast::Sender<HarnessEvent>,
    spent: &std::sync::atomic::AtomicU64,
) -> Result<(), String> {
    for job in jobs {
        let answered = if backend
            .helpers
            .per_prompt_micros
            .is_some_and(|cap| spent.load(std::sync::atomic::Ordering::Relaxed) >= cap)
        {
            Err(Failure {
                message: "the helpers' budget for this prompt is spent".to_owned(),
                ..Failure::default()
            })
        } else {
            magi_model::noted!(
                "helpers: {} job {} for {} starting",
                job.kind,
                job.id,
                job.role
            );
            run_accounted(job, backend).await
        };
        let done = match answered {
            Ok(answer) => {
                magi_model::noted!(
                    "helpers: {} job {} ran on {}, {} in, {} out",
                    job.kind,
                    job.id,
                    answer.model,
                    answer.usage.prompt_tokens(),
                    answer.usage.output
                );
                spent.fetch_add(
                    answer.usage.cost_micros,
                    std::sync::atomic::Ordering::Relaxed,
                );
                let _ = events.send(HarnessEvent::HelperSpent {
                    role: job.role.clone(),
                    model: answer.model.clone(),
                    usage: answer.usage,
                });
                serde_json::json!({
                    "id": job.id, "text": answer.text, "model": answer.model,
                    "usage": {
                        "input": answer.usage.input, "output": answer.usage.output,
                        "cache_read": answer.usage.cache_read,
                        "cache_write": answer.usage.cache_write,
                        "cost_micros": answer.usage.cost_micros,
                    },
                })
            }
            Err(why) => {
                if why.usage != magi_proto::Usage::default() {
                    spent.fetch_add(why.usage.cost_micros, std::sync::atomic::Ordering::Relaxed);
                    let _ = events.send(HarnessEvent::HelperSpent {
                        role: job.role.clone(),
                        model: why.model.clone(),
                        usage: why.usage,
                    });
                }
                magi_model::noted!(
                    "helpers: {} job {} failed: {}",
                    job.kind,
                    job.id,
                    why.message
                );
                serde_json::json!({ "id": job.id, "failed": why.message,
                    "model": why.model, "usage": why.usage })
            }
        };
        let mut open = scribe.lock().await;
        if let Some(open) = open.as_mut() {
            open.job_done(done)
                .await
                .map_err(|why| format!("helper completion was not recorded: {why}"))?;
        }
    }
    Ok(())
}

/// Background jobs a layout handed out, run beside the turn that asked for them.
pub(crate) fn alongside(
    tasks: &crate::settling::Tasks,
    jobs: Vec<Job>,
    backend: Backend,
    scribe: crate::scribe::Held,
    events: tokio::sync::broadcast::Sender<HarnessEvent>,
    spent: Spend,
) {
    if jobs.is_empty() {
        return;
    }
    if let Err(why) =
        tasks.spawn(async move { work(&jobs, &backend, &scribe, &events, &spent).await })
    {
        magi_model::noted!("helpers: {why}");
    }
}

/// Between turns: hand balthasar what settled, and run the background jobs it has waiting. Spawned,
/// so the person is never waiting on a summary somebody else asked for.
pub async fn between(
    session: std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
    backend: Backend,
    scribe: crate::scribe::Held,
) {
    let (tasks, mut jobs, events, spent) = {
        let mut held = session.lock().await;
        (
            held.helpers(),
            held.take_deferred(),
            held.publisher(),
            held.helpers_spent(),
        )
    };
    if let Err(why) = tasks.spawn(async move {
        crate::scribe::flush(&session, &mut *scribe.lock().await)
            .await
            .map_err(|why| why.to_string())?;
        let waiting = {
            let mut open = scribe.lock().await;
            match open.as_mut() {
                Some(open) => match open.jobs().await {
                    Ok(jobs) => jobs,
                    Err(magi_ipc::family::Fault::Refused(_)) => Vec::new(),
                    Err(why) => return Err(why.to_string()),
                },
                None => return Ok(()),
            }
        };
        for job in waiting
            .into_iter()
            .filter_map(|job| serde_json::from_value::<Job>(job).ok())
        {
            if !jobs.iter().any(|known| known.id == job.id) {
                jobs.push(job);
            }
        }
        if jobs.is_empty() {
            return Ok(());
        }
        work(&jobs, &backend, &scribe, &events, &spent).await
    }) {
        magi_model::noted!("helpers: {why}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn helpers() -> Helpers {
        Helpers {
            roles: [("memory".to_owned(), "local/small".to_owned())].into(),
            ..Helpers::default()
        }
    }

    #[test]
    fn a_role_with_a_helper_runs_on_it() {
        let job = Job {
            role: "memory".into(),
            ..Job::default()
        };
        assert_eq!(
            helpers().model_for(&job, "big").as_deref(),
            Some("local/small")
        );
    }

    #[test]
    fn a_role_of_memorys_is_memorys_until_somebody_names_it() {
        let asks = |role: &str| Job {
            role: role.into(),
            ..Job::default()
        };
        let mut set = helpers();
        for role in ["summary", "notes", "curate"] {
            assert_eq!(
                set.model_for(&asks(role), "big").as_deref(),
                Some("local/small")
            );
        }
        set.roles.insert("summary".into(), "strong/one".into());
        assert_eq!(
            set.model_for(&asks("summary"), "big").as_deref(),
            Some("strong/one")
        );
        assert_eq!(
            set.model_for(&asks("notes"), "big").as_deref(),
            Some("local/small")
        );
        // A role that is not memory's borrows nothing from it.
        assert_eq!(set.model_for(&asks("safety"), "big"), None);
        // With memory off, a summary still has the fallback its job asks for, and notes have none.
        set.roles.clear();
        let mut summary = asks("summary");
        summary.fallback = "main".into();
        assert_eq!(set.model_for(&summary, "big").as_deref(), Some("big"));
        assert_eq!(set.model_for(&asks("notes"), "big"), None);
    }

    #[test]
    fn a_role_without_one_falls_back_only_when_asked_to() {
        let mut job = Job {
            role: "safety".into(),
            ..Job::default()
        };
        assert_eq!(helpers().model_for(&job, "big"), None, "skipped by default");
        job.fallback = "main".into();
        assert_eq!(helpers().model_for(&job, "big").as_deref(), Some("big"));
    }

    #[test]
    fn a_role_set_to_main_runs_on_the_sessions_model() {
        let mut set = helpers();
        set.roles.insert("memory".into(), MAIN.into());
        let job = Job {
            role: "memory".into(),
            ..Job::default()
        };
        assert_eq!(set.model_for(&job, "big").as_deref(), Some("big"));
        assert_eq!(
            set.model_for(&job, ""),
            None,
            "no model of its own to run on"
        );
    }

    #[test]
    fn a_job_says_how_long_it_may_take_before_the_configuration_does() {
        let mut job = Job::default();
        let mut configured = helpers();
        assert_eq!(
            configured.patience(&job).as_millis(),
            u128::from(TIMEOUT_MS)
        );
        configured.timeout_ms = 5_000;
        assert_eq!(configured.patience(&job).as_millis(), 5_000);
        job.timeout_ms = Some(700);
        assert_eq!(configured.patience(&job).as_millis(), 700);
    }

    #[test]
    fn a_job_reads_from_the_wire_with_every_field_optional() {
        let job: Job = serde_json::from_value(serde_json::json!({
            "id": "J-1", "kind": "summarise", "role": "memory", "blocking": true,
        }))
        .expect("a job");
        assert!(job.blocking);
        assert_eq!(job.schema, None);
    }

    #[test]
    fn a_review_is_the_session_models_to_make() {
        let job = Job {
            role: "main".into(),
            fallback: "skip".into(),
            ..Job::default()
        };
        assert_eq!(helpers().model_for(&job, "big").as_deref(), Some("big"));
    }

    pub(super) fn backend(mind: &magi_testkit::Mind, helpers: Helpers) -> Backend {
        Backend {
            tools: Vec::new(),
            clients: Vec::new(),
            tooling: magi_tools::supplier::Tooling::default(),
            cwd: std::env::temp_dir(),
            grants: Vec::new(),
            environ: std::collections::BTreeMap::new(),
            confine: false,
            isolate: false,
            model: "main/model".into(),
            mind: mind.program().display().to_string(),
            wants: magi_proto::ask::Wants::default(),
            context_window: None,
            max_output: None,
            system: None,
            helpers,
        }
    }

    /// Run one memory job against a fake model with `spent` already gone, and say whether it ran.
    async fn ran(cap: u64, spent: u64) -> bool {
        let mind = magi_testkit::Mind::answering(&format!("helping-cap-{cap}-{spent}"), "done");
        let capped = Helpers {
            per_prompt_micros: Some(cap),
            ..helpers()
        };
        let none: crate::scribe::Held = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let (events, mut heard) = tokio::sync::broadcast::channel(8);
        let job = Job {
            id: "J-1".into(),
            role: "memory".into(),
            ..Job::default()
        };
        work(
            &[job],
            &backend(&mind, capped),
            &none,
            &events,
            &std::sync::atomic::AtomicU64::new(spent),
        )
        .await
        .expect("helper work settled");
        matches!(heard.try_recv(), Ok(HarnessEvent::HelperSpent { .. }))
    }

    /// One memory job beside a turn, against a fake model, with the prompt's budget at `spent`.
    async fn beside(label: &str, spent: u64) -> bool {
        let mind = magi_testkit::Mind::answering(label, "done");
        let capped = Helpers {
            per_prompt_micros: Some(5),
            ..helpers()
        };
        let none: crate::scribe::Held = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let (events, mut heard) = tokio::sync::broadcast::channel(8);
        let job = Job {
            id: "J-1".into(),
            role: "memory".into(),
            ..Job::default()
        };
        let budget = Spend::new(std::sync::atomic::AtomicU64::new(spent));
        let tasks = crate::settling::Tasks::default();
        alongside(
            &tasks,
            vec![job],
            backend(&mind, capped),
            none,
            events,
            budget,
        );
        tasks
            .pause()
            .expect("pause helpers")
            .drain(std::time::Duration::from_secs(10))
            .await
            .expect("helpers settled");
        matches!(heard.try_recv(), Ok(HarnessEvent::HelperSpent { .. }))
    }

    #[tokio::test]
    async fn a_job_beside_the_turn_is_waited_for_and_held_to_the_budget() {
        assert!(
            beside("helping-beside-free", 0).await,
            "settled did not wait for it"
        );
        assert!(
            !beside("helping-beside-spent", 5).await,
            "a job beside the turn ran on a prompt that had spent its budget"
        );
    }

    #[test]
    fn a_helper_is_asked_not_to_reason() {
        let mind = magi_testkit::Mind::answering("helping-thinking", "done");
        let shaped = Job {
            schema: Some(serde_json::json!({ "type": "object", "required": ["ops"] })),
            ..Job::default()
        };
        let asked = wants(&shaped, &backend(&mind, helpers()), "local/small");
        assert_eq!(asked.thinking, Some(magi_model::ThinkingLevel::Off));
        assert_eq!(asked.max_tokens, Some(MAX_TOKENS));
        assert_eq!(
            asked.schema, None,
            "the shape is asked for in words, not forced"
        );
    }

    #[test]
    fn the_shape_an_answer_takes_is_said_in_the_instruction() {
        let shaped = Job {
            instruction: "Keep notes.".into(),
            schema: Some(serde_json::json!({ "required": ["ops"] })),
            ..Job::default()
        };
        let said = instructed(&shaped).expect("an instruction");
        assert!(said.starts_with("Keep notes."), "{said}");
        assert!(said.contains(r#""required":["ops"]"#), "{said}");
        assert_eq!(
            instructed(&Job::default()),
            None,
            "nothing to say is nothing sent"
        );
    }

    #[tokio::test]
    async fn a_prompt_whose_helper_budget_is_spent_runs_no_more_jobs() {
        assert!(ran(1_000, 0).await, "under the cap, the job runs");
        assert!(!ran(1_000, 1_000).await, "at the cap, it is not started");
    }
}
