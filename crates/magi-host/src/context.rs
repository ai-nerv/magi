//! Rebuilding the conversation the provider is shown. The journal holds what happened; a provider
//! needs what was said. Everything here is a view: sessions are append-only, so nothing in this
//! file removes anything — it decides what to look at.

use crate::session::Session;
use magi_model::{Content, Context, Message, Role, StopReason};
use magi_proto::Entry;

/// Build the provider-facing conversation from the transcript. Tool entries become tool results,
/// and an assistant entry that failed is dropped rather than replayed as if the model had said it.
pub fn of(session: &Session) -> Context {
    of_entries(session.entries())
}

/// The same, over entries the caller chose. Compaction needs it: it has to summarise exactly the
/// entries it declares replaced. Entry counts and message counts agree only when every entry makes
/// one message, and a `Notice`, `Branch`, `Compaction` or errored assistant entry makes none.
#[must_use]
pub fn of_entries(entries: &[Entry]) -> Context {
    let view = live_entries(entries);
    let summary = view.summary;
    let masks = view.masks;
    // Carried with the entry, because a mask names an entry by its index in the transcript.
    let live = view.live.into_iter().map(|at| (at, &entries[at]));

    let mut messages: Vec<Message> = Vec::new();
    if let Some(summary) = summary {
        // As a user message: a model shown its own words as a summary tends to continue them.
        messages.push(Message::user(format!(
            "Here is a summary of the earlier part of this conversation:\n\n{summary}"
        )));
    }

    // Where the assistant message being rebuilt lives, so the tool entries after it can put their
    // calls back into it. The journal stores a call as its own record; a provider needs it inside.
    let mut open: Option<usize> = None;

    for (at, entry) in live {
        match entry {
            // A notice is one UI talking to the person in front of it. A mask is bookkeeping about
            // another entry; what it carries is applied where that entry is written out, below.
            Entry::Branch { .. }
            | Entry::Compaction { .. }
            | Entry::Notice { .. }
            | Entry::Masked { .. } => {}
            Entry::User { text, aside, .. } => {
                open = None;
                // The aside goes with it, under a rule, so the model can tell it from the prompt.
                messages.push(Message::user(if aside.is_empty() {
                    text.clone()
                } else {
                    format!("{text}\n\n---\n{aside}")
                }));
            }
            // Somebody addressed this session, so it is a user turn — but not the user. Named
            // rather than dropped: swallowing a message another agent sent is worth no tidiness.
            Entry::From { who, kin, text, .. } => {
                open = None;
                messages.push(Message::user(format!(
                    "[message from {}::{who}]\n{text}",
                    kin.to_uppercase()
                )));
            }
            Entry::Assistant {
                text,
                thinking,
                stop_reason,
                error,
                signatures,
                ..
            } => {
                open = None;
                // Replaying an error as if the model had said it teaches it to produce more.
                if error.is_some() || *stop_reason == Some(StopReason::Error) {
                    continue;
                }
                let mut content = Vec::new();
                if !thinking.is_empty() {
                    content.push(Content::Thinking {
                        thinking: thinking.clone(),
                        signature: signatures.thinking.clone(),
                    });
                }
                if !text.is_empty() {
                    content.push(Content::Text {
                        text: text.clone(),
                        signature: signatures.text.clone(),
                    });
                }
                // Pushed even when empty: a tool-using turn is a model that says nothing and calls.
                messages.push(Message {
                    role: Role::Assistant,
                    content,
                    stop_reason: *stop_reason,
                    usage: None,
                    error: None,
                });
                open = Some(messages.len() - 1);
            }
            Entry::Tool {
                id,
                name,
                args,
                result,
                thought_signature,
            } => {
                if let Some(at) = open {
                    messages[at].content.push(Content::ToolCall {
                        id: id.to_string(),
                        name: name.clone(),
                        arguments: serde_json::from_str(args).unwrap_or(serde_json::Value::Null),
                        thought_signature: thought_signature.clone(),
                    });
                }
                if let Some(result) = result {
                    // Where masking saves the window. The stub is the tool's own words, because
                    // only its author knows what a useful one says. The call above is never masked:
                    // a result without its call is an orphan, and providers refuse those.
                    let content = masks
                        .get(&at)
                        .cloned()
                        .unwrap_or_else(|| result.output.clone());
                    messages.push(Message {
                        role: Role::Tool,
                        content: vec![Content::ToolResult {
                            id: id.to_string(),
                            name: name.clone(),
                            content,
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

    // A message with nothing in it is rejected by every provider that checks.
    messages.retain(|m| !(m.role == Role::Assistant && m.content.is_empty()));
    Context {
        messages: repair(messages),
        ..Context::default()
    }
}

/// What a call that was never answered is told to the model as. An error result, because a call the
/// model is shown as unanswered is a call it will sit and wait for.
const NEVER_ANSWERED: &str =
    "no result was recorded for this call — the session ended while the tool was running";

/// Make the conversation one a provider will accept. Two shapes break it, and both come back as
/// [`magi_proto::ask::Refusal::Invalid`] — neither retryable nor `Overflow` — so nothing recovers.
/// A result with no call is dropped, since the call it answers is gone; a rewind can still place a
/// cut there. A call with no result gets a synthesised error result, because dropping the call
/// would rewrite what the model said — Anthropic rejects an unanswered `tool_use`.
fn repair(messages: Vec<Message>) -> Vec<Message> {
    let answered: std::collections::BTreeSet<String> = messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|content| match content {
            Content::ToolResult { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();

    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    // A result is matched against the calls before it: one that arrives first has nothing to answer.
    let mut called: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for mut message in messages {
        if message.role == Role::Tool {
            message.content.retain(|content| match content {
                Content::ToolResult { id, .. } => called.contains(id),
                _ => true,
            });
            if !message.content.is_empty() {
                out.push(message);
            }
            continue;
        }

        let missing: Vec<(String, String)> = message
            .content
            .iter()
            .filter_map(|content| match content {
                Content::ToolCall { id, name, .. } if !answered.contains(id) => {
                    Some((id.clone(), name.clone()))
                }
                _ => None,
            })
            .collect();
        for content in &message.content {
            if let Content::ToolCall { id, .. } = content {
                called.insert(id.clone());
            }
        }
        out.push(message);
        // Straight after the message that made them, which is where a provider looks for them.
        for (id, name) in missing {
            out.push(Message {
                role: Role::Tool,
                content: vec![Content::ToolResult {
                    id,
                    name,
                    content: NEVER_ANSWERED.to_owned(),
                    is_error: true,
                }],
                stop_reason: None,
                usage: None,
                error: None,
            });
        }
    }
    out
}

/// What a view of the transcript comes to.
struct Live {
    /// Indices into the transcript, in the order a provider is shown them.
    live: Vec<usize>,
    /// The summary standing in for whatever a compaction replaced.
    summary: Option<String>,
    /// What a masked entry is sent as instead of itself, by index.
    masks: std::collections::BTreeMap<usize, String>,
}

/// The entries the provider is shown, the summary standing in for the rest, and the stubs. One
/// pass: compactions and branches both answer which entries are live, they compose, and both count
/// in entries from the start of the session. A mask does not change which entries are live, only
/// what one of them says. Nothing is removed from the journal by any of them — this is a view.
fn live_entries(entries: &[Entry]) -> Live {
    let mut live: Vec<usize> = Vec::new();
    let mut summary = None;
    let mut masks: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
    for (at, entry) in entries.iter().enumerate() {
        match entry {
            // Everything after the branch point stops being live. The entries stay.
            Entry::Branch { keeps, .. } => live.retain(|&i| i < *keeps),
            Entry::Compaction {
                summary: text,
                replaces,
                ..
            } => {
                summary = Some(text.clone());
                live.retain(|&i| i >= *replaces);
            }
            // The record itself is not sent; what it carries is applied where the entry is written.
            Entry::Masked {
                at: which, shown, ..
            } => {
                masks.insert(*which, shown.clone());
            }
            _ => live.push(at),
        }
    }
    Live {
        live,
        summary,
        masks,
    }
}

/// Where "undo the last exchange" rewinds to: the last live user message, not the last journalled
/// one, or rewinding twice would do nothing the second time. `None` when there is nothing to undo.
#[must_use]
pub fn rewind_point(entries: &[Entry]) -> Option<usize> {
    live_entries(entries)
        .live
        .into_iter()
        .rev()
        .find(|&i| matches!(entries[i], Entry::User { .. }))
}

/// The last thing the person actually asked, among what is still live — what a recall is keyed on.
/// `None` when nothing has been asked, which is a session that has only been listened to.
#[must_use]
pub fn last_asked(session: &Session) -> Option<String> {
    let entries = session.entries();
    let at = rewind_point(entries)?;
    match &entries[at] {
        Entry::User { text, .. } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use magi_journal::JournalError;
    use magi_model::scratch::Scratch;
    use magi_proto::{MessageId, SessionId, Signatures, ToolCallId, ToolResult};

    fn session(name: &str) -> (Session, Scratch) {
        let dir = Scratch::new("magi-ctx", name);
        let session = Session::recorded(SessionId::new("s"), Vec::new());
        (session, dir)
    }

    /// One tool-using round, exactly as the turn loop journals it.
    fn tool_round(session: &mut Session) -> Result<(), JournalError> {
        session.commit(Entry::User {
            id: MessageId::new("u1"),
            text: "read the file".into(),
            aside: String::new(),
        })?;
        session.commit(Entry::Assistant {
            id: MessageId::new("a2"),
            text: String::new(),
            thinking: "I should read it".into(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            signatures: Signatures {
                text: None,
                thinking: Some("sig-thinking".into()),
            },
            usage: magi_proto::Usage::default(),
        })?;
        session.commit(Entry::Tool {
            id: ToolCallId::new("c1"),
            name: "read".into(),
            args: r#"{"path":"a.rs"}"#.into(),
            result: Some(ToolResult {
                output: "contents".into(),
                is_error: false,
                shown: None,
            }),
            thought_signature: Some("sig-call".into()),
        })?;
        Ok(())
    }

    #[test]
    fn the_call_the_model_made_is_replayed_with_its_result() {
        // An OpenAI-compatible endpoint takes an orphaned result and leaves the model with no
        // record of what it asked for, which is worse because it looks like it worked.
        let (mut session, _dir) = session("callback");
        tool_round(&mut session).expect("journal");

        let context = of(&session);
        let assistant = context
            .messages
            .iter()
            .find(|m| m.role == Role::Assistant)
            .expect("an assistant message");
        let call = assistant
            .content
            .iter()
            .find_map(|c| match c {
                Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => Some((id, name, arguments)),
                _ => None,
            })
            .expect("the assistant asked for a tool, so the message must show it");
        assert_eq!(call.0, "c1");
        assert_eq!(call.1, "read");
        assert_eq!(call.2["path"], "a.rs");
    }

    #[test]
    fn the_call_comes_before_the_result_it_answers() {
        let (mut session, _dir) = session("order");
        tool_round(&mut session).expect("journal");

        let context = of(&session);
        let asked = context
            .messages
            .iter()
            .position(|m| {
                m.content
                    .iter()
                    .any(|c| matches!(c, Content::ToolCall { .. }))
            })
            .expect("a call");
        let answered = context
            .messages
            .iter()
            .position(|m| {
                m.content
                    .iter()
                    .any(|c| matches!(c, Content::ToolResult { .. }))
            })
            .expect("a result");
        assert!(asked < answered, "{asked} came after {answered}");
    }

    #[test]
    fn the_signatures_survive_the_journal() {
        // A reasoning model sends a token standing for its reasoning; dropping it is a 400 on the
        // second round trip of every tool-using turn.
        let (mut session, _dir) = session("signatures");
        tool_round(&mut session).expect("journal");

        let context = of(&session);
        let assistant = context
            .messages
            .iter()
            .find(|m| m.role == Role::Assistant)
            .expect("an assistant message");
        let thinking = assistant
            .content
            .iter()
            .find_map(|c| match c {
                Content::Thinking { signature, .. } => Some(signature.clone()),
                _ => None,
            })
            .expect("a thinking block");
        assert_eq!(thinking.as_deref(), Some("sig-thinking"));

        let carried = assistant.content.iter().find_map(|c| match c {
            Content::ToolCall {
                thought_signature, ..
            } => Some(thought_signature.clone()),
            _ => None,
        });
        assert_eq!(carried.flatten().as_deref(), Some("sig-call"));
    }

    #[test]
    fn a_message_that_only_asked_for_a_tool_is_still_a_message() {
        // The common shape: the model says nothing and calls something. Dropping it takes the call.
        let (mut session, _dir) = session("silent");
        session
            .commit(Entry::Assistant {
                id: MessageId::new("a1"),
                text: String::new(),
                thinking: String::new(),
                stop_reason: Some(StopReason::ToolUse),
                error: None,
                signatures: Signatures::default(),
                usage: magi_proto::Usage::default(),
            })
            .expect("journal");
        session
            .commit(Entry::Tool {
                id: ToolCallId::new("c1"),
                name: "read".into(),
                args: "{}".into(),
                result: Some(ToolResult {
                    output: "x".into(),
                    is_error: false,
                    shown: None,
                }),
                thought_signature: None,
            })
            .expect("journal");

        let context = of(&session);
        assert!(
            context.messages.iter().any(|m| m
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall { .. }))),
            "{:?}",
            context.messages
        );
    }

    #[test]
    fn an_assistant_message_with_nothing_at_all_is_still_dropped() {
        // The empty entry the turn loop commits before the first delta; providers reject it.
        let (mut session, _dir) = session("empty");
        session
            .commit(Entry::Assistant {
                id: MessageId::new("a1"),
                text: String::new(),
                thinking: String::new(),
                stop_reason: None,
                error: None,
                signatures: Signatures::default(),
                usage: magi_proto::Usage::default(),
            })
            .expect("journal");
        assert!(of(&session).messages.is_empty());
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;
    use magi_model::scratch::Scratch;
    use magi_proto::{MessageId, SessionId, Signatures};

    fn session(name: &str) -> (Session, Scratch) {
        let dir = Scratch::new("magi-branch", name);
        let session = Session::recorded(SessionId::new("s"), Vec::new());
        (session, dir)
    }

    fn exchange(session: &mut Session, n: usize) {
        session
            .commit(Entry::User {
                id: MessageId::new(format!("u{n}")),
                text: format!("question {n}"),
                aside: String::new(),
            })
            .expect("commit");
        session
            .commit(Entry::Assistant {
                id: MessageId::new(format!("a{n}")),
                text: format!("answer {n}"),
                thinking: String::new(),
                stop_reason: Some(StopReason::EndTurn),
                error: None,
                signatures: Signatures::default(),
                usage: magi_proto::Usage::default(),
            })
            .expect("commit");
    }

    #[test]
    fn a_branch_hides_what_came_after_it_without_deleting_it() {
        let (mut session, _dir) = session("hide");
        exchange(&mut session, 1);
        exchange(&mut session, 2);
        session
            .commit(Entry::Branch {
                id: MessageId::new("b1"),
                keeps: 2,
            })
            .expect("commit");

        let sent = format!("{:?}", of(&session).messages);
        assert!(sent.contains("question 1"), "{sent}");
        assert!(!sent.contains("question 2"), "{sent}");
        // Append-only: what happened is still in the journal and still on screen.
        assert_eq!(session.entries().len(), 5);
    }

    #[test]
    fn the_rewind_point_is_the_last_live_message_not_the_last_one() {
        // Counting from the journal would name a message the first rewind already dropped.
        let (mut session, _dir) = session("twice");
        exchange(&mut session, 1);
        exchange(&mut session, 2);

        let first = rewind_point(session.entries()).expect("a point");
        assert_eq!(first, 2, "the second question");
        session
            .commit(Entry::Branch {
                id: MessageId::new("b1"),
                keeps: first,
            })
            .expect("commit");

        let second = rewind_point(session.entries()).expect("a point");
        assert_eq!(second, 0, "the first question, not the second again");
    }

    #[test]
    fn rewinding_an_empty_session_has_nowhere_to_go() {
        let (session, _dir) = session("empty");
        assert_eq!(rewind_point(session.entries()), None);
    }

    #[test]
    fn a_branch_and_a_compaction_compose() {
        // Both answer which entries are live, so they have to agree.
        let (mut session, _dir) = session("compose");
        for n in 1..=6 {
            exchange(&mut session, n);
        }
        session
            .commit(Entry::Compaction {
                id: MessageId::new("k1"),
                summary: "six questions were asked".into(),
                replaces: 8,
            })
            .expect("commit");
        session
            .commit(Entry::Branch {
                id: MessageId::new("b1"),
                keeps: 10,
            })
            .expect("commit");

        let sent = format!("{:?}", of(&session).messages);
        assert!(
            sent.contains("six questions were asked"),
            "the summary: {sent}"
        );
        assert!(!sent.contains("question 1"), "summarised away: {sent}");
        assert!(
            sent.contains("question 5"),
            "kept by the compaction: {sent}"
        );
        assert!(
            !sent.contains("question 6"),
            "dropped by the branch: {sent}"
        );
    }
}

#[cfg(test)]
mod repairing;

#[cfg(test)]
#[path = "context/masking.rs"]
mod masking;
