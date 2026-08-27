//! Making room when the conversation outgrows the window.
//!
//! A session stops working long before it stops being useful: the window fills, and every
//! further prompt is refused. The usual fix is to drop the oldest messages, which throws away
//! exactly the part that said what the task was — so this replaces them with a summary the
//! model writes for itself, and keeps the recent exchanges verbatim because that is where the
//! detail that still matters lives.
//!
//! Nothing is deleted. Sessions are append-only, the transcript on screen is unchanged, and a
//! compaction is a record saying what the *provider* is now sent.

use axum_model::{Content, Context, Message, Role};
use axum_provider::model::Model;

/// The percentage of the window at which a conversation is compacted.
///
/// Well short of full, because compacting costs a request of its own and that request needs
/// room to run. Leaving it until the window is nearly gone means the summarisation itself is
/// the thing that overflows.
const HIGH_WATER_PERCENT: usize = 75;

/// Messages kept verbatim at the end of a compacted conversation.
///
/// The recent turns are where the detail still in play lives — the file just read, the error
/// just seen — and a summary of them is always worse than having them. Older than this and a
/// summary is usually better than the raw text.
pub const KEEP: usize = 8;

/// Characters per token, near enough.
///
/// An estimate rather than a tokeniser: every provider counts differently, a tokeniser is a
/// per-vendor dependency, and being wrong here costs one early compaction. The reactive path
/// — the provider replying that the window overflowed — catches whatever this misses, so this
/// only has to be roughly right.
const CHARS_PER_TOKEN: usize = 4;

/// Roughly how many tokens a context will cost to send.
#[must_use]
pub fn estimate(context: &Context) -> usize {
    let mut chars = context.system.as_deref().map_or(0, str::len);
    for message in &context.messages {
        for content in &message.content {
            chars += match content {
                Content::Text { text, .. } => text.len(),
                Content::Thinking { thinking, .. } => thinking.len(),
                Content::ToolCall {
                    name, arguments, ..
                } => name.len() + arguments.to_string().len(),
                Content::ToolResult { content, .. } => content.len(),
                Content::Image { data, .. } => data.len(),
            };
        }
    }
    for tool in &context.tools {
        chars += tool.name.len() + tool.description.len() + tool.parameters.to_string().len();
    }
    chars / CHARS_PER_TOKEN
}

/// Whether this context should be compacted before it is sent.
#[must_use]
pub fn needed(context: &Context, model: &Model) -> bool {
    let window = usize::try_from(model.context_window).unwrap_or(usize::MAX);
    // A model that declares no window cannot be over it. Better than treating zero as full
    // and compacting on every turn.
    // Multiplied out rather than divided, so a small window does not round its threshold to
    // zero and compact every turn.
    window > 0 && estimate(context).saturating_mul(100) > window.saturating_mul(HIGH_WATER_PERCENT)
}

/// How many entries at the front of the transcript a compaction should cover.
///
/// Counted in journal entries rather than in messages, because that is what the record stores
/// and what the replay skips. Returns `None` when there is not enough history to be worth
/// summarising — compacting four messages into a paragraph saves nothing and loses detail.
#[must_use]
pub fn covers(entries: usize) -> Option<usize> {
    // One more than `KEEP` so a compaction always removes at least one entry; otherwise a
    // session at the high-water mark compacts on every turn and never gets smaller.
    (entries > KEEP + 1).then(|| entries - KEEP)
}

/// The conversation to summarise, and the instruction for doing it.
///
/// A context of its own rather than an extra message on the real one: the summariser is not
/// continuing the conversation, and tools it might call have no meaning here.
#[must_use]
pub fn request(context: &Context, through: usize) -> Context {
    let mut messages: Vec<Message> = context.messages.iter().take(through).cloned().collect();
    messages.push(Message {
        role: Role::User,
        content: vec![Content::Text {
            text: INSTRUCTION.to_owned(),
            signature: None,
        }],
        stop_reason: None,
        usage: None,
        error: None,
    });
    Context {
        messages,
        system: None,
        tools: Vec::new(),
    }
}

/// What the model is asked to produce.
///
/// Specific about what to keep, because a general "summarise this" returns prose about the
/// conversation rather than the facts the next turn needs: which files were touched, what was
/// decided, what is still outstanding.
const INSTRUCTION: &str = "\
Summarise the conversation above so that it can be continued without it.

Write it for the assistant that will read it, not for a person. Keep:

- what the user is trying to achieve, in their own terms
- decisions that were made, and anything explicitly ruled out
- files, commands and identifiers that were read, written or run, by name
- what has been done so far, and what is left

Leave out pleasantries, restatements and anything already superseded. Do not add a preamble \
or a closing remark — the summary itself is the whole reply.";

#[cfg(test)]
mod tests {
    use super::*;

    fn model(window: u64) -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            provider: "p".into(),
            api: axum_provider::model::Api::OpenAiCompletions,
            reasoning: false,
            input: Vec::new(),
            context_window: window,
            max_tokens: 100,
            cost: axum_model::Cost::default(),
            thinking: std::collections::BTreeMap::new(),
            compat: None,
        }
    }

    fn context(messages: usize, each: usize) -> Context {
        Context {
            messages: (0..messages)
                .map(|_| Message::user("x".repeat(each)))
                .collect(),
            ..Context::default()
        }
    }

    #[test]
    fn a_small_conversation_is_left_alone() {
        assert!(!needed(&context(2, 100), &model(200_000)));
    }

    #[test]
    fn a_conversation_past_the_high_water_mark_is_compacted() {
        // 100 messages of 4,000 characters is ~100k tokens against a 100k window.
        assert!(needed(&context(100, 4_000), &model(100_000)));
    }

    #[test]
    fn a_conversation_just_under_the_mark_is_left_alone() {
        // The case the extremes miss. 700 messages of 400 characters is ~70k tokens against a
        // 100k window: under 75%, so nothing happens. An earlier version compared
        // `estimate * 100` against `window * 75 / 100` and compacted here.
        assert!(!needed(&context(700, 400), &model(100_000)));
    }

    #[test]
    fn a_conversation_just_over_the_mark_is_compacted() {
        assert!(needed(&context(800, 400), &model(100_000)));
    }

    #[test]
    fn a_model_declaring_no_window_is_never_over_it() {
        // Zero would otherwise read as "full", and every turn would compact.
        assert!(!needed(&context(100, 4_000), &model(0)));
    }

    #[test]
    fn there_is_nothing_to_compact_in_a_short_session() {
        assert_eq!(covers(3), None);
        assert_eq!(covers(KEEP), None);
        assert_eq!(covers(KEEP + 1), None);
    }

    #[test]
    fn compacting_always_removes_at_least_one_entry() {
        // Otherwise a session at the mark compacts every turn and never gets smaller.
        let covered = covers(KEEP + 2).expect("something to compact");
        assert!(covered >= 1);
        assert_eq!(covered, 2);
    }

    #[test]
    fn the_summariser_is_given_no_tools() {
        // It is not continuing the conversation, and a tool call from it would be journalled
        // as part of a turn that is not happening.
        let asked = request(&context(10, 10), 5);
        assert!(asked.tools.is_empty());
        assert_eq!(asked.messages.len(), 6, "five kept, one instruction");
    }

    #[test]
    fn the_instruction_asks_for_what_the_next_turn_needs() {
        // A general "summarise this" returns prose about the conversation instead of the
        // facts that let it continue.
        for wanted in ["files", "decisions", "left"] {
            assert!(INSTRUCTION.contains(wanted), "{wanted}");
        }
    }
}
