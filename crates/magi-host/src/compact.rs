//! Making room when the conversation outgrows the window: the oldest messages become a summary the
//! model writes for itself, recent exchanges stay verbatim, and nothing is deleted. Whether to
//! compact is balthasar's (`crate::turn::memory`); where the cut may legally fall is [`legal`].

use magi_model::{Content, Context, Message, Role};
use magi_proto::Entry;

/// The cut balthasar asked for, moved forward to where it may legally fall. `None` when everything
/// from `asked` on is tool results: a cut splitting a call from its result is a 400 the retry
/// classifier calls `Invalid`, which nothing recovers from.
#[must_use]
pub fn legal(entries: &[Entry], asked: usize) -> Option<usize> {
    let mut cut = asked.min(entries.len());
    while entries.get(cut).is_some_and(is_answer) {
        cut += 1;
    }
    (cut > 0 && cut < entries.len()).then_some(cut)
}

fn is_answer(entry: &Entry) -> bool {
    matches!(entry, Entry::Tool { .. })
}

/// The conversation to summarise and the instruction for it, taken whole rather than as a count.
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

/// What the model is asked to produce, specific because "summarise this" returns prose instead.
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
