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
        system: Some(job.instruction.clone()).filter(|s| !s.is_empty()),
        messages: vec![magi_model::Message::user(job.input.clone())],
        tools: Vec::new(),
    };
    let wants =
        magi_proto::ask::Wants {
            thinking: None,
            max_tokens: Some(job.max_tokens.unwrap_or(MAX_TOKENS)),
            schema: job.schema.clone().filter(|s| !s.is_null()).map(|schema| {
                magi_proto::ask::Schema {
                    name: if job.kind.is_empty() {
                        "answer".to_owned()
                    } else {
                        job.kind.clone()
                    },
                    schema,
                }
            }),
            // The routing chosen for the session's model means nothing to another one.
            provider: (model == backend.model)
                .then(|| backend.wants.provider.clone())
                .flatten(),
        };
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
                return Err(format!("{model} answered nothing"));
            }
            Ok(Answer {
                text,
                model,
                usage: turn.usage(),
            })
        }
    }
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
            run(job, backend).await
        };
        let done = match answered {
            Ok(answer) => {
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
        let waiting = {
            let mut open = scribe.lock().await;
            match open.as_mut() {
                Some(open) => open.jobs().await.unwrap_or_default(),
                None => return,
            }
        };
        let jobs: Vec<Job> = waiting
            .into_iter()
            .filter_map(|job| serde_json::from_value(job).ok())
            .collect();
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
}
