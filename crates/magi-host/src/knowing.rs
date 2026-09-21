//! Answering what a surface asks about the session — the other half of [`crate::holder`], which
//! hands a tenant rows and forwards what the person does. A question goes out on a channel and the
//! answer comes back on one, and the blocking side waits with a deadline. Nothing here writes,
//! names a path, or reaches the network.

use magi_proto::wondering::{Answered, Wonder};

/// How long a tenant waits for an answer before being told there is not one. Short: it is holding
/// the screen, and every question here is a lookup against something already open.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// One question on its way to the session, and the way back.
pub struct Wondering {
    pub wonder: Wonder,
    pub args: serde_json::Value,
    /// Where the answer goes. A std channel, because the waiting end is a blocking thread.
    pub back: std::sync::mpsc::Sender<Answered>,
}

/// What magi can say about itself, for a surface that asks. What a session is answers without
/// leaving the thread; everything live is read where it lives rather than copied and left to stale.
pub struct Knows {
    session: String,
    cwd: String,
    /// The way to anything that has to be awaited. `None` where nothing is listening.
    asking: Option<tokio::sync::mpsc::UnboundedSender<Wondering>>,
}

impl Knows {
    /// What magi knows about a session in `cwd`, before anything asks it anything.
    #[must_use]
    pub fn of(session: &magi_proto::SessionId, cwd: &str) -> Self {
        Self {
            session: session.as_str().to_owned(),
            cwd: cwd.to_owned(),
            asking: None,
        }
    }

    /// Send the questions it cannot answer itself down `asking`.
    #[must_use]
    pub fn asking(mut self, asking: tokio::sync::mpsc::UnboundedSender<Wondering>) -> Self {
        self.asking = Some(asking);
        self
    }

    /// Put a question to whatever is listening, and wait for the answer.
    fn ask_along(
        &self,
        wonder: Wonder,
        args: &serde_json::Value,
        patience: std::time::Duration,
    ) -> Answered {
        let Some(asking) = &self.asking else {
            return refused(wonder, "this session has nothing to ask");
        };
        let (back, answer) = std::sync::mpsc::channel();
        let sent = asking.send(Wondering {
            wonder,
            args: args.clone(),
            back,
        });
        if sent.is_err() {
            return refused(wonder, "the session is no longer listening");
        }
        answer
            .recv_timeout(patience)
            .unwrap_or_else(|_| refused(wonder, "nothing answered in time"))
    }
}

/// How long a surface waits for a helper model. A model answering, not a lookup.
const HELPING: std::time::Duration = std::time::Duration::from_secs(60);

impl magi_tools::holding::Answers for Knows {
    fn answer(&self, wonder: Wonder, args: &serde_json::Value) -> Answered {
        match wonder {
            Wonder::Session if self.asking.is_none() => Answered::Told {
                said: serde_json::json!({ "id": self.session, "cwd": self.cwd }),
            },
            Wonder::Session => {
                let mut answer = self.ask_along(wonder, args, PATIENCE);
                if let Answered::Told { said } = &mut answer {
                    said["cwd"] = serde_json::json!(self.cwd);
                }
                answer
            }
            Wonder::Model | Wonder::Memories => self.ask_along(wonder, args, PATIENCE),
            Wonder::Helper => self.ask_along(wonder, args, HELPING),
        }
    }
}

/// A refusal that names the verb it refused, so a tenant never has to show a bare "no".
#[must_use]
pub fn refused(wonder: Wonder, because: &str) -> Answered {
    Answered::Refused {
        because: format!("{}: {because}", wonder.verb()),
    }
}

/// Serve questions from `asked` out of `scribe`, until nothing is asking any more. Its own task,
/// because a question belongs to the session and not to whichever UI was attached when it arrived.
pub async fn serve(
    mut asked: tokio::sync::mpsc::UnboundedReceiver<Wondering>,
    scribe: std::sync::Arc<tokio::sync::Mutex<Option<crate::scribe::Scribe>>>,
    session: std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
    backend: Option<crate::turn::Backend>,
) {
    while let Some(asking) = asked.recv().await {
        let answered = match asking.wonder {
            Wonder::Session => {
                let held = session.lock().await;
                Answered::Told {
                    said: serde_json::json!({ "id": held.id() }),
                }
            }
            Wonder::Memories => memories(&scribe, &asking.args).await,
            Wonder::Model => model(&session).await,
            // Answered on a task of its own: a model takes seconds, and the next question should not
            // queue behind it.
            Wonder::Helper => {
                let tasks = session.lock().await.helpers();
                let session = std::sync::Arc::clone(&session);
                let backend = backend.clone();
                let _ = tasks.spawn(async move {
                    let answered = helper(&session, backend, &asking.args).await;
                    let _ = asking.back.send(answered);
                    Ok(())
                });
                continue;
            }
        };
        let _ = asking.back.send(answered);
    }
}

/// Put a surface's question to the helper model for the role it named. The session's own model
/// stands in only when the question says `fallback: "main"`.
async fn helper(
    session: &tokio::sync::Mutex<crate::session::Session>,
    backend: Option<crate::turn::Backend>,
    args: &serde_json::Value,
) -> Answered {
    let Some(mut backend) = backend else {
        return refused(Wonder::Helper, "this session has no model to help with");
    };
    let text = |key: &str| {
        args.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let job = crate::helping::Job {
        id: String::new(),
        kind: "helper".to_owned(),
        role: text("role"),
        fallback: text("fallback"),
        instruction: text("instruction"),
        input: text("input"),
        schema: args.get("schema").cloned(),
        max_tokens: args.get("max_tokens").and_then(serde_json::Value::as_u64),
        blocking: true,
        timeout_ms: args.get("timeout_ms").and_then(serde_json::Value::as_u64),
        structured: args.get("structured").and_then(serde_json::Value::as_bool) == Some(true),
        thinking: args
            .get("thinking")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    };
    let (events, asked_for) = {
        let held = session.lock().await;
        // `/model` replaces the model mid-session; the fallback is the one answering now.
        if let Some(name) = held.model_name() {
            backend.model = name;
        }
        (held.publisher(), said_by_the_person(held.entries()))
    };
    // What the person said, and only that, for a question that asks for it: what a tool printed
    // is what such a question must not be swayed by.
    let mut job = job;
    if args.get("said").and_then(serde_json::Value::as_bool) == Some(true) {
        job.input = format!("{asked_for}\n{}", job.input);
    }
    match crate::helping::run_accounted(&job, &backend).await {
        Ok(answer) => {
            let _ = events.send(magi_proto::HarnessEvent::HelperSpent {
                role: job.role.clone(),
                model: answer.model.clone(),
                usage: answer.usage,
            });
            Answered::Told {
                said: serde_json::json!({ "text": answer.text, "model": answer.model }),
            }
        }
        Err(why) => {
            if why.usage != magi_proto::Usage::default() {
                let _ = events.send(magi_proto::HarnessEvent::HelperSpent {
                    role: job.role.clone(),
                    model: why.model,
                    usage: why.usage,
                });
            }
            refused(Wonder::Helper, &why.message)
        }
    }
}

/// The newest things the person said, oldest first, within a bound: what they asked for, and any
/// line they drew. Nothing an assistant wrote or a tool printed.
fn said_by_the_person(entries: &[magi_proto::Entry]) -> String {
    const KEPT: usize = 8;
    const OF_EACH: usize = 600;
    let said: Vec<String> = entries
        .iter()
        .filter_map(|entry| match entry {
            magi_proto::Entry::User { text, .. } => Some(text.chars().take(OF_EACH).collect()),
            _ => None,
        })
        .collect();
    let mut out = String::from("What the person has said, oldest first:\n");
    for text in &said[said.len().saturating_sub(KEPT)..] {
        out.push_str(&format!("- {text}\n"));
    }
    out
}

/// The model answering here, read where it lives: `/model` replaces it mid-session, and a copy
/// taken at startup would name one nothing is talking to.
async fn model(session: &tokio::sync::Mutex<crate::session::Session>) -> Answered {
    match session.lock().await.model() {
        Some(model) => Answered::Told {
            said: serde_json::json!({
                "name": model.name,
                "context_window": model.context_window,
            }),
        },
        None => refused(Wonder::Model, "no model is answering here"),
    }
}

/// What balthasar holds, for the query the surface named.
async fn memories(
    scribe: &tokio::sync::Mutex<Option<crate::scribe::Scribe>>,
    args: &serde_json::Value,
) -> Answered {
    let query = args
        .get("query")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 100);
    let mut held = scribe.lock().await;
    let Some(scribe) = held.as_mut() else {
        // The ordinary case on a machine without balthasar, and not an error: a surface can say
        // "nothing remembers here" rather than draw an empty list.
        return refused(Wonder::Memories, "there is no balthasar in this session");
    };
    match scribe.nearest(query, limit).await {
        // The memories alone; the injection id is bookkeeping between magi and balthasar.
        Ok(found) => Answered::Told {
            said: serde_json::Value::Array(found.memories),
        },
        Err(why) => refused(Wonder::Memories, &why.to_string()),
    }
}

/// What the memory layer says about its notes, for a screen that asked. After anything that
/// changes them, the notes and their log follow as they now stand, so one ask redraws the view.
pub async fn notes(
    scribe: &crate::scribe::Held,
    verb: &str,
    arg: serde_json::Value,
) -> Vec<magi_proto::HarnessEvent> {
    const ASKABLE: &[&str] = &["notes", "changes", "note_open", "undo", "approve", "reject"];
    let refusal = |message: String| magi_proto::HarnessEvent::Refused {
        cursor: magi_proto::Cursor::ZERO,
        message,
    };
    if !ASKABLE.contains(&verb) {
        return vec![refusal(format!("`{verb}` is not a question about notes"))];
    }
    magi_model::noted!("notes: a screen asked for {verb}");
    let mut asks = vec![(verb.to_owned(), arg)];
    if !matches!(verb, "note_open" | "changes") {
        if verb != "notes" {
            asks.push(("notes".to_owned(), serde_json::json!({})));
        }
        asks.push(("changes".to_owned(), serde_json::json!({ "limit": 50 })));
    }
    let mut open = scribe.lock().await;
    let Some(open) = open.as_mut() else {
        return vec![refusal("there is no balthasar in this session".to_owned())];
    };
    let mut out = Vec::new();
    for (verb, arg) in asks {
        out.push(
            match tokio::time::timeout(PATIENCE, open.ask_all(&verb, arg)).await {
                Ok(Ok(rows)) => magi_proto::HarnessEvent::MemoryAnswered {
                    verb,
                    answer: serde_json::Value::Array(rows),
                },
                Ok(Err(why)) => refusal(format!("{verb}: {why}")),
                Err(_) => refusal(format!("{verb}: balthasar did not answer in time")),
            },
        );
    }
    out
}

/// What balthasar's float asks for, each verb in the shape it actually takes. Separate from
/// [`notes`], whose verbs are all `(session, …)` and which asks for their neighbours besides.
pub async fn held(
    scribe: &crate::scribe::Held,
    verb: &str,
    arg: &serde_json::Value,
) -> magi_proto::HarnessEvent {
    let refusal = |message: String| magi_proto::HarnessEvent::Refused {
        cursor: magi_proto::Cursor::ZERO,
        message,
    };
    let mut open = scribe.lock().await;
    let Some(open) = open.as_mut() else {
        return refusal("there is no balthasar in this session".to_owned());
    };
    magi_model::noted!("memory: a screen asked for {verb}");
    let asked = async {
        match verb {
            "sessions" => open.sessions().await,
            "why" => open.why(arg.as_str().unwrap_or_default()).await,
            "recall" => {
                let limit = arg["limit"].as_u64().unwrap_or(50);
                let query = arg["query"].as_str().unwrap_or_default();
                open.browsing(query, limit).await
            }
            _ => Err(magi_ipc::family::Fault::Refused(format!(
                "`{verb}` is not one of the memory float's questions"
            ))),
        }
    };
    match tokio::time::timeout(PATIENCE, asked).await {
        Ok(Ok(rows)) => magi_proto::HarnessEvent::MemoryAnswered {
            verb: verb.to_owned(),
            answer: serde_json::Value::Array(rows),
        },
        Ok(Err(why)) => refusal(format!("{verb}: {why}")),
        Err(_) => refusal(format!("{verb}: balthasar did not answer in time")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_tools::holding::Answers;

    #[test]
    fn a_session_says_which_one_it_is_and_where_it_runs() {
        let knows = Knows::of(&magi_proto::SessionId::new("s-1"), "/tmp/project");
        let Answered::Told { said } = knows.answer(Wonder::Session, &serde_json::Value::Null)
        else {
            panic!("a session always knows this much");
        };
        assert_eq!(said["id"], "s-1");
        assert_eq!(said["cwd"], "/tmp/project");
    }

    #[test]
    fn a_question_with_nowhere_to_go_is_refused_rather_than_waited_on() {
        let knows = Knows::of(&magi_proto::SessionId::new("s-1"), "/tmp");
        let began = std::time::Instant::now();
        let Answered::Refused { because } = knows.answer(Wonder::Memories, &serde_json::json!({}))
        else {
            panic!("there is nothing to ask");
        };
        assert!(because.starts_with("memories:"), "{because}");
        assert!(began.elapsed() < PATIENCE, "it waited for an answer");
    }

    /// A session with nothing recorded in it, for the questions that need one.
    fn a_session() -> std::sync::Arc<tokio::sync::Mutex<crate::session::Session>> {
        std::sync::Arc::new(tokio::sync::Mutex::new(crate::session::Session::recorded(
            magi_proto::SessionId::new("s-1"),
            Vec::new(),
        )))
    }

    #[tokio::test]
    async fn a_connected_surface_reads_the_resumed_session_instead_of_its_cached_id() {
        let (asking, asked) = tokio::sync::mpsc::unbounded_channel();
        let session = a_session();
        let serving = tokio::spawn(serve(
            asked,
            std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            std::sync::Arc::clone(&session),
            None,
        ));
        session
            .lock()
            .await
            .resume_prepared(magi_journal::Journal::recorded(
                magi_proto::SessionId::new("B"),
                Vec::new(),
            ));
        let knows = Knows::of(&magi_proto::SessionId::new("s-1"), "/tmp/project").asking(asking);
        let answered = tokio::task::spawn_blocking(move || {
            knows.answer(Wonder::Session, &serde_json::Value::Null)
        })
        .await
        .expect("answer task");
        assert!(
            matches!(answered, Answered::Told {said} if said["id"] == "B" && said["cwd"] == "/tmp/project")
        );
        serving.await.expect("surface server stopped");
    }

    #[tokio::test]
    async fn a_session_with_no_balthasar_says_so_by_name() {
        let (asking, asked) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(serve(
            asked,
            std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            a_session(),
            None,
        ));
        let knows = Knows::of(&magi_proto::SessionId::new("s-1"), "/tmp").asking(asking);
        let answered = tokio::task::spawn_blocking(move || {
            knows.answer(Wonder::Memories, &serde_json::json!({ "query": "deploy" }))
        })
        .await
        .expect("the question was put");
        let Answered::Refused { because } = answered else {
            panic!("there is no balthasar");
        };
        assert!(because.contains("balthasar"), "{because}");
    }

    #[tokio::test]
    async fn the_model_answered_is_the_one_answering_now() {
        let session = a_session();
        let (asking, asked) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(serve(
            asked,
            std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            std::sync::Arc::clone(&session),
            None,
        ));
        let knows = std::sync::Arc::new(
            Knows::of(&magi_proto::SessionId::new("s-1"), "/tmp").asking(asking),
        );

        let asked_first = {
            let knows = std::sync::Arc::clone(&knows);
            tokio::task::spawn_blocking(move || {
                knows.answer(Wonder::Model, &serde_json::Value::Null)
            })
            .await
            .expect("the question was put")
        };
        let Answered::Refused { because } = asked_first else {
            panic!("nothing has named a model yet");
        };
        assert!(because.starts_with("model:"), "{because}");

        session.lock().await.set_model(Some(magi_proto::ModelInfo {
            name: "haiku".to_owned(),
            context_window: 200_000,
        }));
        let asked_again = tokio::task::spawn_blocking(move || {
            knows.answer(Wonder::Model, &serde_json::Value::Null)
        })
        .await
        .expect("the question was put");
        let Answered::Told { said } = asked_again else {
            panic!("a model was named");
        };
        assert_eq!(said["name"], "haiku");
        assert_eq!(said["context_window"], 200_000);
    }

    #[tokio::test]
    async fn a_screen_may_ask_only_about_notes_and_hears_when_nothing_keeps_them() {
        let none: crate::scribe::Held = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let refused = |events: &[magi_proto::HarnessEvent], wanted: &str| matches!(events, [magi_proto::HarnessEvent::Refused { message, .. }] if message.contains(wanted));
        let odd = notes(&none, "observe", serde_json::json!({})).await;
        assert!(refused(&odd, "not a question about notes"), "{odd:?}");
        let asked = notes(&none, "notes", serde_json::json!({})).await;
        assert!(refused(&asked, "no balthasar"), "{asked:?}");
    }
}
