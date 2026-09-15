//! Small models run on somebody else's behalf. balthasar wants a summary or a tidy-up, a surface
//! wants to know whether something is safe; magi owns the providers, so magi runs them — with the
//! model `magi.helpers` names for the role — hands back what came, and says what it cost.

use crate::catalog::Backend;
use magi_proto::HarnessEvent;

pub use crate::catalog::Helpers;
pub use magi_proto::laying::Job;

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

impl Helpers {
    /// The model to run `job` with, or `None` when nothing should.
    #[must_use]
    pub fn model_for(&self, job: &Job, main: &str) -> Option<String> {
        // `main` is the session's own model by name: a review is the main model's to make.
        if job.role == "main" && !main.is_empty() {
            return Some(main.to_owned());
        }
        self.roles
            .get(&job.role)
            .cloned()
            .or_else(|| (job.fallback == "main" && !main.is_empty()).then(|| main.to_owned()))
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
    let model = backend
        .helpers
        .model_for(job, &backend.model)
        .ok_or_else(|| format!("no helper is configured for `{}`", job.role))?;
    let context = magi_model::Context {
        system: instructed(job),
        messages: vec![magi_model::Message::user(job.input.clone())],
        tools: Vec::new(),
    };
    let wants = wants(job, backend, &model);
    let patience = backend.helpers.patience(job);

    let mut turn = magi_core::Turn::new();
    let mut args = String::new();
    let asked = tokio::time::timeout(
        patience,
        crate::broker::ask_through(
            &backend.mind,
            &model,
            &context,
            &wants,
            |delta| {
                // Anthropic answers a schema by calling a forced tool, so its arguments are the text.
                if let magi_model::Delta::ToolCallArgs(chunk) = &delta {
                    args.push_str(chunk);
                }
                turn.apply(delta);
            },
            |_| {},
        ),
    )
    .await;
    match asked {
        Err(_) => Err(format!("{model} did not answer within {patience:?}")),
        Ok(Err(trouble)) => Err(trouble.message),
        Ok(Ok(())) => {
            let text = if args.trim().is_empty() {
                turn.text().trim().to_owned()
            } else {
                args.trim().to_owned()
            };
            if text.is_empty() {
                let thought = turn.thinking().chars().count();
                return Err(if thought > 0 {
                    format!("{model} spent its tokens thinking ({thought} chars) and said nothing")
                } else {
                    format!("{model} answered nothing")
                });
            }
            Ok(Answer {
                text,
                model,
                usage: turn.usage(),
            })
        }
    }
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
    spent: &mut u64,
) {
    for job in jobs {
        let answered = if backend
            .helpers
            .per_prompt_micros
            .is_some_and(|cap| *spent >= cap)
        {
            Err("the helpers' budget for this prompt is spent".to_owned())
        } else {
            magi_model::noted!(
                "helpers: {} job {} for {} starting",
                job.kind,
                job.id,
                job.role
            );
            run(job, backend).await
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
                *spent += answer.usage.cost_micros;
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
                magi_model::noted!("helpers: {} job {} failed: {why}", job.kind, job.id);
                serde_json::json!({ "id": job.id, "failed": why })
            }
        };
        let mut open = scribe.lock().await;
        if let Some(open) = open.as_mut()
            && let Err(why) = open.job_done(done).await
        {
            magi_model::noted!("helpers: job_done was refused: {why}");
        }
    }
}

/// Between turns: hand balthasar what settled, and run the background jobs it has waiting. Spawned,
/// so the person is never waiting on a summary somebody else asked for.
pub fn between(
    session: std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
    backend: Backend,
    scribe: crate::scribe::Held,
) {
    tokio::spawn(async move {
        if let Err(why) = crate::scribe::flush(&session, &mut *scribe.lock().await).await {
            magi_model::noted!("helpers: the transcript could not be handed over: {why}");
            return;
        }
        // What the turn's layouts handed out first, then whatever else balthasar has waiting.
        let mut jobs = session.lock().await.take_deferred();
        let waiting = {
            let mut open = scribe.lock().await;
            match open.as_mut() {
                Some(open) => open.jobs().await.unwrap_or_default(),
                None => return,
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
            return;
        }
        let events = session.lock().await.publisher();
        work(&jobs, &backend, &scribe, &events, &mut 0).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helpers() -> Helpers {
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

    fn backend(mind: &magi_testkit::Mind, helpers: Helpers) -> Backend {
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
        work(&[job], &backend(&mind, capped), &none, &events, &mut {
            spent
        })
        .await;
        matches!(heard.try_recv(), Ok(HarnessEvent::HelperSpent { .. }))
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
        assert_eq!(asked.schema, None, "the shape is asked for in words, not forced");
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
        assert_eq!(instructed(&Job::default()), None, "nothing to say is nothing sent");
    }

    #[tokio::test]
    async fn a_prompt_whose_helper_budget_is_spent_runs_no_more_jobs() {
        assert!(ran(1_000, 0).await, "under the cap, the job runs");
        assert!(!ran(1_000, 1_000).await, "at the cap, it is not started");
    }
}
