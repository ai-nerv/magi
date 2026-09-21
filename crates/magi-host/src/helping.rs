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

/// The roles that fall back to `memory` when nobody named them: one line in a configuration keeps
/// working, and a person who wants a stronger model for summaries than for notes says so for that
/// role alone. Four are balthasar's jobs; `search` is a tool's, and is here because it is the same
/// kind of work — a small model reading text it is handed and saying something short about it.
const OF_MEMORY: &[&str] = &["summary", "notes", "curate", "contradict", "search"];

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

    /// Which roles this session can actually run a job for, as balthasar asks about them by name.
    /// Derived rather than listed: `memory` alone can run every job balthasar has, through the
    /// fallbacks above, and a harness that named only what was written down would say it could
    /// run none of them. A session whose conversation stays out of the project's notes says it
    /// cannot run the ones that write them, which is how that promise is kept.
    #[must_use]
    pub fn runnable(&self, main: &str) -> Vec<String> {
        const ASKED_ABOUT: &[&str] = &[
            "memory",
            "summary",
            "notes",
            "curate",
            "contradict",
            "safety",
        ];
        const WRITES_NOTES: &[&str] = &["memory", "notes", "curate", "contradict"];
        ASKED_ABOUT
            .iter()
            .filter(|role| !(self.no_notes && WRITES_NOTES.contains(role)))
            .filter(|role| {
                let job = Job {
                    role: (*role).to_string(),
                    ..Job::default()
                };
                self.model_for(&job, main).is_some()
            })
            .map(|role| (*role).to_owned())
            .collect()
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
    let decides = backend.decides(&model);
    let context = magi_model::Context {
        system: instructed(job, decides),
        messages: vec![magi_model::Message::user(job.input.clone())],
        tools: Vec::new(),
    };
    let wants = wants(job, backend, &model, decides);
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

/// How much a job may reason. None unless it asked, because a helper is there to be quick and
/// cheap and one left to reason spent a whole budget thinking and answered nothing — measured,
/// on the sweep for contradictions: 999 of 1000 tokens reasoned, no answer. The other half is
/// measured too: extraction with nothing to think with returned an empty list every time.
fn thinking(job: &Job) -> magi_model::ThinkingLevel {
    job.thinking
        .as_deref()
        .and_then(|level| serde_json::from_value(serde_json::Value::String(level.to_owned())).ok())
        .unwrap_or(magi_model::ThinkingLevel::Off)
}

/// What a job asks its model for.
fn wants(job: &Job, backend: &Backend, model: &str, decides: bool) -> magi_proto::ask::Wants {
    magi_proto::ask::Wants {
        thinking: Some(thinking(job)),
        max_tokens: Some(job.max_tokens.unwrap_or(MAX_TOKENS)),
        // In words, unless the job allows a schema and the model answers a schema and nothing
        // else: one that writes and is held to a schema from the first token has nowhere to
        // think but inside the strings, and wrote its reasoning into a note's title.
        schema: (job.structured && decides).then(|| shaped(job)).flatten(),
        // The routing chosen for the session's model means nothing to another one.
        provider: (model == backend.model)
            .then(|| backend.wants.provider.clone())
            .flatten(),
    }
}

/// The schema as a provider takes one, for a job that asked for it that way.
fn shaped(job: &Job) -> Option<magi_proto::ask::Schema> {
    let schema = job.schema.clone().filter(|schema| !schema.is_null())?;
    Some(magi_proto::ask::Schema {
        name: job.kind.clone(),
        schema,
    })
}

/// A job's instruction, with the shape its answer must take said in words when it has one.
fn instructed(job: &Job, decides: bool) -> Option<String> {
    // In words only where it does not go as a schema; both at once is one thing said twice.
    let shape = job
        .schema
        .as_ref()
        .filter(|schema| !(schema.is_null() || job.structured && decides));
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
    fn what_a_session_says_it_can_run_is_what_it_can_actually_run() {
        // One line naming `memory` runs every job balthasar has, so it says so for all of them:
        // it is the name balthasar gates note-keeping on, and a session that listed only what
        // was written down would have said it could keep none.
        let mut set = helpers();
        assert_eq!(
            set.runnable("big"),
            ["memory", "summary", "notes", "curate", "contradict"],
            "and not `safety`, which nothing named"
        );
        set.roles.insert("safety".into(), "judge/one".into());
        assert!(set.runnable("big").contains(&"safety".to_owned()));
        // Named alone, without `memory`, it is still run: a person who named a notes model
        // meant it to be used.
        let only_notes = Helpers {
            roles: [("notes".to_owned(), "cheap/one".to_owned())].into(),
            ..Helpers::default()
        };
        assert_eq!(only_notes.runnable("big"), ["notes"]);
        // Nothing named and no model: nothing is claimed.
        assert!(Helpers::default().runnable("").is_empty());
        // A session whose conversation stays out of the notes cannot run what writes them.
        let child = Helpers {
            no_notes: true,
            ..helpers()
        };
        assert_eq!(child.runnable("big"), ["summary"]);
    }

    #[test]
    fn a_role_of_memorys_is_memorys_until_somebody_names_it() {
        let asks = |role: &str| Job {
            role: role.into(),
            ..Job::default()
        };
        let mut set = helpers();
        for role in ["summary", "notes", "curate", "contradict"] {
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
            deciders: Vec::new(),
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
        let asked = wants(&shaped, &backend(&mind, helpers()), "local/small", false);
        assert_eq!(asked.thinking, Some(magi_model::ThinkingLevel::Off));
        assert_eq!(asked.max_tokens, Some(MAX_TOKENS));
        assert_eq!(
            asked.schema, None,
            "the shape is asked for in words, not forced"
        );
    }

    /// Both halves were measured, and they disagree, so the job settles it. A narrow question
    /// left to reason spent 999 of 1000 tokens on it and answered nothing; extraction with
    /// nothing to think with answered `{"ops": []}` in six tokens, every time.
    #[test]
    fn a_job_that_has_to_work_something_out_may_say_so() {
        let mind = magi_testkit::Mind::answering("helping-thinking-asked", "done");
        let asking = |level: Option<&str>| {
            let job = Job {
                thinking: level.map(str::to_owned),
                ..Job::default()
            };
            wants(&job, &backend(&mind, helpers()), "local/small", false).thinking
        };
        assert_eq!(asking(None), Some(magi_model::ThinkingLevel::Off));
        assert_eq!(asking(Some("low")), Some(magi_model::ThinkingLevel::Low));
        assert_eq!(
            asking(Some("nonsense")),
            Some(magi_model::ThinkingLevel::Off),
            "an unreadable level is no reason to let a helper reason without limit"
        );
    }

    #[test]
    fn a_model_that_only_decides_is_sent_the_shape_and_one_that_writes_is_told_it() {
        let mind = magi_testkit::Mind::answering("helping-shape", "done");
        let job = Job {
            kind: "verdict".into(),
            instruction: "Is it safe?".into(),
            schema: Some(serde_json::json!({ "type": "object", "required": ["safe"] })),
            structured: true,
            ..Job::default()
        };
        let backend = backend(&mind, helpers());
        // One that writes: in words, as every other helper job has it.
        assert_eq!(wants(&job, &backend, "local/small", false).schema, None);
        assert!(
            instructed(&job, false)
                .expect("an instruction")
                .contains("JSON Schema"),
        );
        // One that only decides: as a schema, and then not also in words.
        let sent = wants(&job, &backend, "deciding/one", true)
            .schema
            .expect("a schema");
        assert_eq!(sent.name, "verdict");
        assert_eq!(sent.schema["required"], serde_json::json!(["safe"]));
        let mut decides = job.clone();
        decides.structured = true;
        assert_eq!(
            instructed(&decides, true).as_deref(),
            Some("Is it safe?"),
            "one thing twice is one too many"
        );
        // A job that did not ask is never sent one, whatever answers it.
        let plain = Job {
            structured: false,
            ..job
        };
        assert_eq!(wants(&plain, &backend, "deciding/one", true).schema, None);
    }

    #[test]
    fn the_shape_an_answer_takes_is_said_in_the_instruction() {
        let shaped = Job {
            instruction: "Keep notes.".into(),
            schema: Some(serde_json::json!({ "required": ["ops"] })),
            ..Job::default()
        };
        let said = instructed(&shaped, false).expect("an instruction");
        assert!(said.starts_with("Keep notes."), "{said}");
        assert!(said.contains(r#""required":["ops"]"#), "{said}");
        assert_eq!(
            instructed(&Job::default(), false),
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
