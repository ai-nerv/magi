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
//!
//! # What is decided here, and what is not
//!
//! **Whether to compact, and how much, is balthasar's** — see `crate::turn::memory`. This module
//! used to hold the whole decision: a high-water mark over a character estimate, a constant
//! `KEEP = 8`, and a cut computed from the two. All three are gone. balthasar is looking at the
//! same window, knows what it has already masked and what a tool's output turned out to be worth,
//! and magi asked it what it would do and then ignored the answer.
//!
//! **Where the cut may legally fall is magi's**, and that is [`legal`]. It is not a second opinion
//! about how much to summarise: it is the one thing balthasar cannot know, because it is a fact
//! about the provider wire rather than about the conversation. A cut between an assistant message
//! and the tool result answering it sends the result on its own, Anthropic answers 400, and the
//! retry classifier calls that `Invalid` — neither retryable nor `Overflow`, so nothing recovers.
//! Every long tool-heavy session used to end that way, with `/clear` the only way out.

use magi_model::{Content, Context, Message, Role};
use magi_proto::Entry;

/// The cut balthasar asked for, moved to somewhere it may legally fall.
///
/// Returns `None` when no legal cut exists at or after `asked`, which happens when everything
/// from there to the end is one long run of tool results. Nothing is compacted then: the reactive
/// path — the provider saying the window overflowed — is what catches it, and a broken
/// conversation would not have been an improvement on a full one.
///
/// Forward rather than back. Forward always removes at least as much as was asked for, so the
/// window still shrinks; a cut moved back can reach the start and compact nothing at all.
#[must_use]
pub fn legal(entries: &[Entry], asked: usize) -> Option<usize> {
    let mut cut = asked.min(entries.len());
    while entries.get(cut).is_some_and(is_answer) {
        cut += 1;
    }
    (cut > 0 && cut < entries.len()).then_some(cut)
}

/// Whether this entry is the answer to a call made before it.
fn is_answer(entry: &Entry) -> bool {
    matches!(entry, Entry::Tool { .. })
}

/// The conversation to summarise, and the instruction for doing it.
///
/// A context of its own rather than an extra message on the real one: the summariser is not
/// continuing the conversation, and tools it might call have no meaning here.
///
/// Takes the whole of what it is given rather than a count to cut at. The count was the bug: two
/// counts were computed independently, one in messages and one in entries, and they only agree
/// when every entry makes exactly one message. See [`crate::context::of_entries`].
#[must_use]
pub fn request(context: &Context) -> Context {
    let mut messages: Vec<Message> = context.messages.clone();
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
#[path = "compact/cutting.rs"]
mod cutting;
