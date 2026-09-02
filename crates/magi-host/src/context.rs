//! Rebuilding the conversation the provider is shown.
//!
//! The journal holds what happened; a provider needs what was said. They are not the same
//! thing and the gap between them is where this module lives: a tool call is journalled as its
//! own record but has to be sent inside the message that made it, an errored turn is on screen
//! but must not be replayed as if the model had said it, and a session that has been compacted
//! or rewound shows more than it sends.
//!
//! Everything here is a *view*. Sessions are append-only and delete-never, so nothing in this
//! file removes anything — it decides what to look at.

use crate::session::Session;
use magi_model::{Content, Context, Message, Role, StopReason};
use magi_proto::Entry;

/// Build the provider-facing conversation from the transcript.
///
/// The journal holds what was shown; a provider needs what was said. Tool entries become tool
/// results, and an assistant entry that failed is dropped — replaying an error as if the model
/// had said it teaches it to produce more of them.
pub fn of(session: &Session) -> Context {
    let entries = session.entries();
    let (live, summary) = live_entries(entries);
    let live = live.into_iter().map(|i| &entries[i]);

    let mut messages: Vec<Message> = Vec::new();
    if let Some(summary) = summary {
        // As a user message, because it is context the model is being given rather than
        // something it said. A model shown its own words as a summary tends to continue them.
        messages.push(Message::user(format!(
            "Here is a summary of the earlier part of this conversation:\n\n{summary}"
        )));
    }

    // Where the assistant message currently being rebuilt lives, so the tool entries that
    // follow it can put their calls back into it. The journal stores a call as its own record
    // -- it is committed before the registry is consulted, which is what makes an unrouted
    // call auditable -- but a provider needs it inside the message that made it.
    let mut open: Option<usize> = None;

    for entry in live {
        match entry {
            // A notice is one UI talking to the person in front of it. Sending it to a
            // provider would be telling the model what magi told somebody about magi.
            Entry::Branch { .. } | Entry::Compaction { .. } | Entry::Notice { .. } => {}
            Entry::User { text, aside, .. } => {
                open = None;
                // The aside goes with it, under a rule, so the model can tell what the person
                // said from what the harness knew. Nobody sees this but the model — the
                // transcript shows the prompt on its own.
                messages.push(Message::user(if aside.is_empty() {
                    text.clone()
                } else {
                    format!("{text}\n\n---\n{aside}")
                }));
            }
            // Somebody addressed this session, so it is a user turn — but not *the* user, and
            // the difference decides whether the model treats it as an instruction or as
            // something a peer said. Named rather than dropped: silently swallowing a message
            // another agent sent is the one failure worth none of the tidiness.
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
                // Pushed even when empty, because the common shape of a tool-using turn is a
                // model that says nothing and calls something. The empty ones are pruned below.
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

    // The entry committed before the first delta has no content and never gained a call. A
    // message with nothing in it is rejected by every provider that checks.
    messages.retain(|m| !(m.role == Role::Assistant && m.content.is_empty()));
    Context {
        messages: repair(messages),
        ..Context::default()
    }
}

/// What a call that was never answered is told to the model as.
///
/// The turn loop writes "cancelled before this tool ran" for every remaining call when a turn is
/// interrupted, which is the same shape and the honest sentence for that cause. This is the other
/// one: the daemon went away while the tool was still running, so nobody was left to amend the
/// entry. Both are an error result, because a call the model is shown as unanswered is a call it
/// will sit and wait for.
const NEVER_ANSWERED: &str =
    "no result was recorded for this call — the session ended while the tool was running";

/// Make the conversation one a provider will accept.
///
/// Two shapes break it, in opposite directions, and both reach the provider as a 400 that
/// [`magi_provider::retry`] classifies `Invalid` — neither retryable nor `Overflow` — so nothing
/// recovers and `/clear` is the only way out.
///
/// **A result with no call.** A compaction or a branch whose boundary fell between an assistant
/// message and the tool entries answering it. [`crate::compact::covers`] no longer places a cut
/// there, but a rewind can, and a session recorded by an older build already has one. The result
/// is dropped: the call it answers is gone, and there is nothing to attach it to.
///
/// **A call with no result.** An `Entry::Tool` committed before the registry was consulted and
/// never amended, which is what a daemon killed mid-tool leaves behind. Verified by doing it, and
/// verified honestly: OpenRouter accepted the orphan and answered, so this is latent and
/// provider-dependent rather than live — Anthropic rejects an unanswered `tool_use`. An error
/// result is synthesised, because dropping the call instead would rewrite what the model said.
///
/// This is the pass §8 listed as stolen from Pi's `transform_messages()` and never wrote. Pi's
/// does four jobs; this does the one that breaks conversations. Image downgrade and thinking
/// keep/drop/downgrade belong to whatever needs them, and tool-id rewriting is per dialect, which
/// is the adapters' business rather than this file's.
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
    // Calls seen so far, so a result is matched against the calls *before* it rather than
    // against the whole conversation: a result that arrives first has nothing to answer.
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

/// The entries the provider is shown, and the summary standing in for the rest.
///
/// One pass, because compactions and branches both answer the same question — which entries
/// are still live — and they compose. A branch after a compaction drops the tail of what
/// survived it; a compaction after a branch summarises what the branch left. Both count in
/// entries from the start of the session, so both are answered against the same indices, and
/// neither has to know the other exists.
///
/// Nothing is removed from the journal by either. This is a view.
fn live_entries(entries: &[Entry]) -> (Vec<usize>, Option<String>) {
    let mut live: Vec<usize> = Vec::new();
    let mut summary = None;
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
            _ => live.push(at),
        }
    }
    (live, summary)
}

/// Where "undo the last exchange" rewinds to.
///
/// The last user message that is still live — not the last one in the journal. After a rewind
/// the abandoned exchange is still on screen, so counting from the journal would name a
/// message that is already gone and rewinding twice would do nothing the second time.
///
/// `None` when there is nothing to undo.
#[must_use]
pub fn rewind_point(entries: &[Entry]) -> Option<usize> {
    let (live, _) = live_entries(entries);
    live.into_iter()
        .rev()
        .find(|&i| matches!(entries[i], Entry::User { .. }))
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use magi_journal::JournalError;
    use magi_proto::{MessageId, SessionId, Signatures, ToolCallId, ToolResult};

    fn session(name: &str) -> (Session, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("magi-ctx-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
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
            }),
            thought_signature: Some("sig-call".into()),
        })?;
        Ok(())
    }

    #[test]
    fn the_call_the_model_made_is_replayed_with_its_result() {
        // A tool result with no preceding tool call is not a conversation. Anthropic rejects
        // it outright; an OpenAI-compatible endpoint takes it and leaves the model with no
        // record of what it asked for, which is worse because it looks like it worked.
        let (mut session, dir) = session("callback");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_call_comes_before_the_result_it_answers() {
        let (mut session, dir) = session("order");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_signatures_survive_the_journal() {
        // A reasoning model does not send back reasoning you can re-send; it sends a token
        // standing for it. Dropping it makes the next request a 400 on the providers that
        // check, which is the second round trip of every tool-using turn.
        let (mut session, dir) = session("signatures");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_message_that_only_asked_for_a_tool_is_still_a_message() {
        // Empty text and empty thinking, which is the common shape: the model says nothing and
        // calls something. Dropping it takes the tool call with it.
        let (mut session, dir) = session("silent");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_assistant_message_with_nothing_at_all_is_still_dropped() {
        // The empty entry the turn loop commits before the first delta. Sending it would be
        // a message with no content, which providers reject.
        let (mut session, dir) = session("empty");
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;
    use magi_proto::{MessageId, SessionId, Signatures};

    fn session(name: &str) -> (Session, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("magi-branch-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
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
        let (mut session, dir) = session("hide");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rewind_point_is_the_last_live_message_not_the_last_one() {
        // Rewinding twice must go back twice. Counting from the journal instead of the live
        // view would name a message the first rewind already dropped, and the second would do
        // nothing.
        let (mut session, dir) = session("twice");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewinding_an_empty_session_has_nowhere_to_go() {
        let (session, dir) = session("empty");
        assert_eq!(rewind_point(session.entries()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_branch_and_a_compaction_compose() {
        // Both answer the same question — which entries are live — so they have to agree.
        // A branch after a compaction drops the tail of what survived it.
        let (mut session, dir) = session("compose");
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod repairing;
