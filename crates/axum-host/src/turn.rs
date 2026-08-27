//! Running one turn against a provider.
//!
//! The daemon's half of the loop: it owns the socket, the journal and the clock, and holds no
//! agent logic — [`axum_core::Turn`] decides what happens and this drives it.

use crate::session::Session;
use axum_core::{Step, Turn};
use axum_model::{Content, Context, Message, Role, StopReason};
use axum_proto::{AgentStatus, Entry, MessageId};
use axum_provider::api::Options;
use axum_provider::client::Client;
use axum_provider::model::Model;
use axum_provider::provider::Provider;

/// What the daemon needs to reach a model.
///
/// Plain data, and sendable: the protocol it names is built on the worker's own thread, because
/// a Lua VM is neither `Send` nor `Sync` and cannot be handed over after the fact.
#[derive(Debug, Clone)]
pub struct Backend {
    /// The protocol descriptions to build the VM from, as `(name, source)`.
    ///
    /// Carried as text rather than as a built VM because a VM cannot cross a thread boundary,
    /// and read from the same place the catalog was so that a protocol the user edited is the
    /// one the daemon speaks. Building from the compiled-in copies instead was a bug: an
    /// edited `apis/*.lua` changed what `axum models` reported and nothing else.
    pub apis: Vec<(String, String)>,
    /// The provider offering the model.
    pub provider: Provider,
    /// The model to call.
    pub model: Model,
    /// What to ask for beyond the conversation.
    pub options: Options,
}

/// Run one turn and journal what it produced.
///
/// Deltas are published as they arrive and the entry is amended as it grows, so a UI attaching
/// mid-turn sees the same partial message a UI that was there all along sees. A crash leaves
/// the partial message rather than nothing, which is why the entry is written before it ends.
pub async fn run(
    session: &tokio::sync::Mutex<Session>,
    backend: &Backend,
    adapter: &dyn axum_provider::api::Adapter,
    client: &Client,
) -> Result<(), crate::HostError> {
    let context = context_of(&*session.lock().await);

    {
        let mut held = session.lock().await;
        held.set_status(AgentStatus::Working {
            label: "Thinking".into(),
        });
    }

    let id = MessageId::new(format!("a{}", session.lock().await.cursor().next().0));
    let mut turn = Turn::new();

    // The entry exists before the first delta, so a UI attaching mid-turn has something to
    // extend rather than a message that appears fully formed at the end.
    session.lock().await.commit(assistant(&id, &turn))?;

    let mut deltas = Vec::new();
    let outcome = client
        .stream(
            adapter,
            &backend.provider,
            &backend.model,
            &context,
            &backend.options,
            |delta| deltas.push(delta),
        )
        .await;

    for delta in deltas {
        turn.apply(delta);
    }

    if let Err(error) = outcome {
        // An error is a value, not an exception: the transcript stays well-formed and the UI
        // needs no error branch. Pi's discipline, and the reason its renderer has none.
        turn.abort(StopReason::Error);
        let mut held = session.lock().await;
        held.amend(Entry::Assistant {
            id,
            text: turn.text().to_owned(),
            thinking: turn.thinking().to_owned(),
            stop_reason: Some(StopReason::Error),
            error: Some(error.message),
        })?;
        held.set_status(AgentStatus::Idle);
        return Ok(());
    }

    let mut held = session.lock().await;
    held.amend(assistant(&id, &turn))?;

    // Tools are M3. A turn that asked for them is reported as such rather than left pending,
    // because a status that never changes is indistinguishable from a hang.
    if let Step::RunTools(calls) = turn.step() {
        let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
        held.commit(Entry::Assistant {
            id: MessageId::new("tools-pending"),
            text: String::new(),
            thinking: String::new(),
            stop_reason: Some(StopReason::Error),
            error: Some(format!(
                "the model asked for {} — tools land in M3",
                names.join(", ")
            )),
        })?;
    }
    held.set_status(AgentStatus::Idle);
    Ok(())
}

/// The assistant entry for a turn in its current state.
fn assistant(id: &MessageId, turn: &Turn) -> Entry {
    Entry::Assistant {
        id: id.clone(),
        text: turn.text().to_owned(),
        thinking: turn.thinking().to_owned(),
        stop_reason: match turn.state() {
            axum_core::TurnState::Finished(reason) => Some(reason),
            _ => None,
        },
        error: None,
    }
}

/// Build the provider-facing conversation from the transcript.
///
/// The journal holds what was shown; a provider needs what was said. Tool entries become tool
/// results, and an assistant entry that failed is dropped — replaying an error as if the model
/// had said it teaches it to produce more of them.
pub fn context_of(session: &Session) -> Context {
    let mut messages = Vec::new();
    for entry in session.entries() {
        match entry {
            Entry::User { text, .. } => messages.push(Message::user(text.clone())),
            Entry::Assistant {
                text,
                thinking,
                stop_reason,
                error,
                ..
            } => {
                if error.is_some() || *stop_reason == Some(StopReason::Error) {
                    continue;
                }
                let mut content = Vec::new();
                if !thinking.is_empty() {
                    content.push(Content::Thinking {
                        thinking: thinking.clone(),
                        signature: None,
                    });
                }
                if !text.is_empty() {
                    content.push(Content::Text {
                        text: text.clone(),
                        signature: None,
                    });
                }
                if !content.is_empty() {
                    messages.push(Message {
                        role: Role::Assistant,
                        content,
                        stop_reason: *stop_reason,
                        usage: None,
                        error: None,
                    });
                }
            }
            Entry::Tool {
                id, name, result, ..
            } => {
                if let Some(result) = result {
                    messages.push(Message {
                        role: Role::Tool,
                        content: vec![Content::ToolResult {
                            id: id.to_string(),
                            name: name.clone(),
                            content: result.output.clone(),
                            is_error: result.is_error,
                        }],
                        stop_reason: None,
                        usage: None,
                        error: None,
                    });
                }
            }
        }
    }
    Context {
        messages,
        ..Context::default()
    }
}
