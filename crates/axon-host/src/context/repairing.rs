//! What a repaired message list must look like before it goes to a provider.
//!
//! Split from [`super`] under THE RULE. These are about [`super::repair`] alone: a tool call
//! with no result, a result with no call, an interrupted turn — the shapes a provider rejects,
//! and which of them a session can produce.

mod repair_tests {
    use super::super::*;
    use crate::session::Session;
    use axon_journal::JournalError;
    use axon_proto::{MessageId, SessionId, Signatures, ToolCallId, ToolResult};

    fn session(name: &str) -> (Session, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("axon-repair-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session =
            Session::open(&dir.join("s.jsonl"), SessionId::new("s"), "/tmp", 0).expect("session");
        (session, dir)
    }

    /// An assistant that says nothing and calls something, which is the shape of a tool turn.
    fn calling(session: &mut Session, at: usize) -> Result<axon_proto::Cursor, JournalError> {
        session.commit(Entry::Assistant {
            id: MessageId::new(format!("a{at}")),
            text: String::new(),
            thinking: String::new(),
            stop_reason: Some(StopReason::ToolUse),
            error: None,
            signatures: Signatures::default(),
            usage: axon_proto::Usage::default(),
        })
    }

    /// A call, answered or not.
    fn call(
        session: &mut Session,
        at: usize,
        answered: bool,
    ) -> Result<axon_proto::Cursor, JournalError> {
        session.commit(Entry::Tool {
            id: ToolCallId::new(format!("c{at}")),
            name: "read".into(),
            args: r#"{"path":"a.rs"}"#.into(),
            result: answered.then(|| ToolResult {
                output: "contents".into(),
                is_error: false,
            }),
            thought_signature: None,
        })
    }

    /// Every call in the assembled context, and every result, by id.
    fn calls_and_results(context: &Context) -> (Vec<String>, Vec<String>) {
        let mut calls = Vec::new();
        let mut results = Vec::new();
        for message in &context.messages {
            for content in &message.content {
                match content {
                    Content::ToolCall { id, .. } => calls.push(id.clone()),
                    Content::ToolResult { id, .. } => results.push(id.clone()),
                    _ => {}
                }
            }
        }
        (calls, results)
    }

    /// The assertion the milestone is about: nothing unanswered, nothing unmatched.
    fn assert_sound(context: &Context) {
        let (calls, results) = calls_and_results(context);
        for id in &calls {
            assert!(results.contains(id), "call {id} was never answered");
        }
        for id in &results {
            assert!(calls.contains(id), "result {id} answers no call");
        }
    }

    #[test]
    fn a_call_that_was_never_answered_gets_a_result() {
        // What a daemon killed mid-tool leaves behind: the entry was committed before the
        // registry was consulted -- which is what makes an unrouted call auditable -- and
        // nobody was left to amend it.
        let (mut session, dir) = session("unanswered");
        session
            .commit(Entry::User {
                id: MessageId::new("u1"),
                text: "read it".into(),
                aside: String::new(),
            })
            .expect("journal");
        calling(&mut session, 2).expect("journal");
        call(&mut session, 1, false).expect("journal");

        let context = of(&session);
        assert_sound(&context);
        let answer = context
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .find_map(|c| match c {
                Content::ToolResult {
                    id,
                    content,
                    is_error,
                    ..
                } if id == "c1" => Some((content.clone(), *is_error)),
                _ => None,
            })
            .expect("a synthesised result");
        assert!(answer.1, "and it is an error, not a silent success");
        assert!(answer.0.contains("no result was recorded"), "{}", answer.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_synthesised_result_follows_the_message_that_made_the_call() {
        // Where a provider looks for it. Anywhere else is the same 400 by another route.
        let (mut session, dir) = session("adjacent");
        calling(&mut session, 1).expect("journal");
        call(&mut session, 1, false).expect("journal");

        let context = of(&session);
        let at = context
            .messages
            .iter()
            .position(|m| m.role == Role::Assistant)
            .expect("an assistant message");
        assert_eq!(
            context.messages[at + 1].role,
            Role::Tool,
            "the answer is the very next message"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_result_whose_call_was_summarised_away_is_dropped() {
        // The other direction, and the one that ended every long tool-heavy session: a cut
        // between the assistant message and the entries answering it. `covers` no longer places
        // one there, but a rewind can, and a session recorded by an older build already has one.
        let (mut session, dir) = session("orphan");
        session
            .commit(Entry::User {
                id: MessageId::new("u1"),
                text: "read it".into(),
                aside: String::new(),
            })
            .expect("journal");
        calling(&mut session, 2).expect("journal");
        call(&mut session, 1, true).expect("journal");
        // Replacing the first three entries drops the call and leaves its answer behind.
        session
            .commit(Entry::Compaction {
                id: MessageId::new("k1"),
                summary: "they read a file".into(),
                replaces: 3,
            })
            .expect("journal");

        let context = of(&session);
        assert_sound(&context);
        let (calls, results) = calls_and_results(&context);
        assert!(calls.is_empty() && results.is_empty(), "both went with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_ordinary_round_is_left_exactly_as_it_was() {
        // The repair must be invisible when there is nothing to repair.
        let (mut session, dir) = session("intact");
        calling(&mut session, 1).expect("journal");
        call(&mut session, 1, true).expect("journal");

        let context = of(&session);
        assert_sound(&context);
        let (calls, results) = calls_and_results(&context);
        assert_eq!(calls, vec!["c1"]);
        assert_eq!(results, vec!["c1"]);
        assert_eq!(context.messages.len(), 2, "no message was added");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_calls_in_one_message_are_each_answered() {
        // One assistant message can make several calls, and a provider wants every one closed.
        let (mut session, dir) = session("several");
        calling(&mut session, 1).expect("journal");
        call(&mut session, 1, true).expect("journal");
        call(&mut session, 2, false).expect("journal");
        call(&mut session, 3, false).expect("journal");

        let context = of(&session);
        assert_sound(&context);
        let (calls, _) = calls_and_results(&context);
        assert_eq!(calls.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
