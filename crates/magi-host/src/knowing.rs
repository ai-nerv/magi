//! Answering what a surface asks about the session.
//!
//! The other half of [`crate::holder`]. That one hands a tenant rows and forwards what the person
//! does; this one answers what the tenant asks back. Together they are what makes a surface a
//! participant: before this, everything a tenant knew it had been handed at open.
//!
//! **The awkward shape, a third time.** The tenant is a child process, the thing that asks is a
//! tool on a blocking thread, and two of the three answers live behind an async connection to
//! balthasar. So a question goes out on a channel and the answer comes back on one, exactly as a
//! permission does — and the blocking side waits with a deadline, because a surface holding the
//! screen on an answer that is never coming is the failure this whole layer exists to avoid.
//!
//! What is *not* here is as deliberate as what is. There is no verb that writes, none that names
//! a path, and none that reaches the network: a surface is a picture somebody is looking at, and
//! the things it may ask are the things already on the screen beside it.

use magi_proto::wondering::{Answered, Wonder};

/// How long a tenant waits for an answer before being told there is not one.
///
/// Short. Every question here is a lookup against something already open, so anything slower than
/// this is a sibling in trouble — and the tenant is holding the screen while it waits.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// One question on its way to the session, and the way back.
pub struct Wondering {
    /// What is being asked.
    pub wonder: Wonder,
    /// What the verb was given.
    pub args: serde_json::Value,
    /// Where the answer goes. A std channel, because the waiting end is a blocking thread.
    pub back: std::sync::mpsc::Sender<Answered>,
}

/// What magi can say about itself, for a surface that asks.
///
/// Two kinds of answer, kept apart because they cost different things. What a session *is* — which
/// one it is and where it runs — is fixed for its whole life and answered without leaving the
/// thread. Everything else is live: the model changes when somebody runs `/model`, the memories
/// are balthasar's, and both are read where they actually live rather than copied here and left
/// to go stale.
pub struct Knows {
    /// This session's id.
    session: String,
    /// The directory it runs in.
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
    fn ask_along(&self, wonder: Wonder, args: &serde_json::Value) -> Answered {
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
            .recv_timeout(PATIENCE)
            .unwrap_or_else(|_| refused(wonder, "nothing answered in time"))
    }
}

impl magi_tools::holding::Answers for Knows {
    fn answer(&self, wonder: Wonder, args: &serde_json::Value) -> Answered {
        match wonder {
            Wonder::Session => Answered::Told {
                said: serde_json::json!({ "id": self.session, "cwd": self.cwd }),
            },
            Wonder::Model | Wonder::Memories => self.ask_along(wonder, args),
        }
    }
}

/// A refusal that names the verb it refused.
///
/// Every refusal goes through here, so a tenant putting one on the screen always gets a sentence
/// saying which of its questions went unanswered rather than a bare "no".
#[must_use]
pub fn refused(wonder: Wonder, because: &str) -> Answered {
    Answered::Refused {
        because: format!("{}: {because}", wonder.verb()),
    }
}

/// Serve questions from `asked` out of `scribe`, until nothing is asking any more.
///
/// The one thing on this side that has to be awaited. It runs as its own task rather than as an
/// arm of a connection's loop, because a question belongs to the session and not to whichever UI
/// happened to be attached when a tenant thought of it.
pub async fn serve(
    mut asked: tokio::sync::mpsc::UnboundedReceiver<Wondering>,
    scribe: std::sync::Arc<tokio::sync::Mutex<Option<crate::scribe::Scribe>>>,
    session: std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>,
) {
    while let Some(asking) = asked.recv().await {
        let answered = match asking.wonder {
            Wonder::Memories => memories(&scribe, &asking.args).await,
            Wonder::Model => model(&session).await,
            // Answered without leaving the thread that asked. One arriving here is a verb that
            // grew a source and did not grow a case.
            other => refused(other, "nothing here answers that"),
        };
        let _ = asking.back.send(answered);
    }
}

/// The model answering here, read where it lives.
///
/// Not a copy taken when the session started: `/model` replaces it mid-session, and a surface told
/// the model magi opened with would name one nothing is talking to.
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
        // The ordinary case on a machine without balthasar, and not an error. A surface is told
        // so it can say "nothing remembers here" rather than draw an empty list that reads as
        // "nothing was remembered".
        return refused(Wonder::Memories, "there is no balthasar in this session");
    };
    match scribe.nearest(query, limit).await {
        Ok(found) => Answered::Told {
            said: serde_json::Value::Array(found),
        },
        Err(why) => refused(Wonder::Memories, &why.to_string()),
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
        // No listener, so `memories` has no source. Waiting out the deadline here would hold the
        // screen for five seconds to arrive at the same answer.
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
    async fn a_session_with_no_balthasar_says_so_by_name() {
        let (asking, asked) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(serve(
            asked,
            std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            a_session(),
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
        // Read where it lives rather than copied at startup: `/model` replaces it mid-session, and
        // a surface told the model magi opened with would name one nothing is talking to.
        let session = a_session();
        let (asking, asked) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(serve(
            asked,
            std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            std::sync::Arc::clone(&session),
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
}
