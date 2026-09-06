//! What this project remembers, put in front of the model.
//!
//! **The half of the memory layer that was never connected.** The transcript flows to balthasar
//! — [`crate::scribe`] has done that from the start — and it comes back three ways: a surface
//! can ask through `casper.knows("memories")`, a model can call `recall` as a tool, and
//! `magi doctor` can say the layer is there. All three require somebody to *ask*. Nothing put
//! what the project already knows in front of a turn that never thought to.
//!
//! That is the difference between a memory layer and a search tool. A model that has to know
//! there is something to look up has to have remembered it already.
//!
//! **Asserted and merely known are not the same thing, and the difference is stated.** balthasar
//! decides which a memory is — its `asserted` field, computed against its own confidence floor
//! and handed over rather than left for a caller to work out from a number and a threshold it
//! would have to be told. Above the floor a memory is current truth; below it, it is still
//! searchable, still explained by `why`, and no longer stated as fact. A harness that flattened
//! the two would tell the model that something it was told once in March is true now.
//!
//! Once per prompt, not once per round. A tool-using turn goes round several times and the
//! recall is about what the person asked, not about what the model just read.

use magi_model::{Content, Message, Role};

/// How many memories to ask for.
///
/// More than fit on purpose: the budget decides what is said and this decides what is
/// considered, and asking for exactly what fits would let one long memory crowd out five short
/// ones that would all have gone in.
pub const MOST: u64 = 12;

/// How much of a turn's window may be spent on what it remembers.
///
/// A tenth. Memory competes with the conversation for the same window, and the conversation is
/// what the person is having — an injection large enough to matter is one that pushed out the
/// exchange it was supposed to inform. Small enough that it is never the reason a turn
/// overflows, which is what makes it safe to do unconditionally.
const SHARE: usize = 10;

/// Roughly four characters to the token, which is what the rest of the host estimates with.
const PER_TOKEN: usize = 4;

/// What the model is told before the block, so it can tell recall from conversation.
const PREFACE: &str = "What this project remembers. This is not part of the conversation:";

/// The line that separates what is current from what is merely on record.
const HEDGE: &str = "Also on record, but not current enough to rely on \
                     — check before acting on any of it:";

/// The memories to put in front of a turn, as one message.
///
/// `None` when there is nothing worth saying: no memories, or none that survive the budget. An
/// empty block would be a message that costs tokens to say nothing, every turn.
#[must_use]
pub fn preface(found: &[serde_json::Value], window: usize) -> Option<Message> {
    let budget = window.saturating_mul(SHARE) / 100 * PER_TOKEN;
    if budget == 0 {
        return None;
    }

    let (asserted, known): (Vec<&serde_json::Value>, Vec<&serde_json::Value>) = found
        .iter()
        .filter(|row| !text_of(row).is_empty())
        .partition(|row| {
            row.get("asserted")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        });

    // **The frame is written once and always.** It used to belong to the confident section, so a
    // recall that found only uncertain memories produced a block opening "Also on record…" with
    // nothing to say it was not the conversation — which is the one failure this whole message
    // has to avoid. A model shown recalled text with no frame around it answers it.
    let mut out = format!("{PREFACE}\n");
    let mut spent = PREFACE.len() + 1;
    if spent >= budget {
        return None;
    }

    let mut wrote = fill(&mut out, &mut spent, budget, None, &asserted);
    wrote |= fill(&mut out, &mut spent, budget, Some(HEDGE), &known);

    wrote.then(|| Message {
        role: Role::User,
        content: vec![Content::Text {
            text: out.trim_end().to_owned(),
            signature: None,
        }],
        stop_reason: None,
        usage: None,
        error: None,
    })
}

/// Write as many of `rows` as fit, under `heading`, and say whether any did.
///
/// A heading costs budget too, and is written only when something goes under it: a section title
/// with nothing beneath it tells the model there was nothing, at the price of saying so.
fn fill(
    out: &mut String,
    spent: &mut usize,
    budget: usize,
    heading: Option<&str>,
    rows: &[&serde_json::Value],
) -> bool {
    let mut wrote = false;
    for row in rows {
        let text = text_of(row);
        let line = format!("- {text}\n");
        let cost = line.len()
            + if wrote {
                0
            } else {
                heading.map_or(0, |h| h.len() + 1)
            };
        if *spent + cost > budget {
            break;
        }
        if !wrote && let Some(heading) = heading {
            out.push_str(heading);
            out.push('\n');
        }
        wrote = true;
        out.push_str(&line);
        *spent += cost;
    }
    if wrote {
        out.push('\n');
    }
    wrote
}

/// The text of a recalled memory, trimmed.
fn text_of(row: &serde_json::Value) -> &str {
    row.get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
}

/// Put what is remembered where retrieved context belongs: before the last thing said.
///
/// Not at the front, where a long conversation buries it, and not at the end, where it arrives
/// after the person's own words and reads as something they said. Immediately before the last
/// message is where a model looks for what it was given to answer with.
pub fn put(context: &mut magi_model::Context, remembered: Message) {
    let at = context.messages.len().saturating_sub(1);
    context.messages.insert(at, remembered);
}

#[cfg(test)]
mod tests {
    use super::{PER_TOKEN, SHARE, preface};

    /// A window big enough that nothing is cut, for the tests that are not about the budget.
    const ROOMY: usize = 100_000;

    fn memory(text: &str, asserted: bool) -> serde_json::Value {
        serde_json::json!({ "id": "m1", "text": text, "asserted": asserted, "confidence": 0.9 })
    }

    fn said(message: &magi_model::Message) -> String {
        message
            .content
            .iter()
            .filter_map(|c| match c {
                magi_model::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn what_is_remembered_goes_before_the_last_thing_said() {
        // Not at the front, where a long conversation buries it, and not at the end, where it
        // arrives after the person's own words and reads as something they said.
        let mut context = magi_model::Context {
            messages: vec![
                magi_model::Message::user("an old exchange"),
                magi_model::Message::user("what is the deploy command?"),
            ],
            ..Default::default()
        };
        super::put(&mut context, magi_model::Message::user("REMEMBERED"));
        let texts: Vec<String> = context.messages.iter().map(said).collect();
        assert_eq!(
            texts,
            [
                "an old exchange",
                "REMEMBERED",
                "what is the deploy command?"
            ],
            "retrieved context belongs immediately before the prompt it answers"
        );
    }

    #[test]
    fn nothing_remembered_is_no_message_at_all() {
        // Not an empty block. A message that costs tokens to say nothing, on every turn, is
        // worse than the absence it describes.
        assert!(preface(&[], ROOMY).is_none());
        assert!(preface(&[memory("   ", true)], ROOMY).is_none());
    }

    #[test]
    fn what_is_current_is_stated_and_what_is_not_is_hedged() {
        // The distinction the whole design turns on. balthasar decides which a memory is; this
        // is where the decision is allowed to matter.
        let message = preface(
            &[
                memory("the deploy command is `make ship`", true),
                memory("the staging box is 10.0.0.7", false),
            ],
            ROOMY,
        )
        .expect("two memories");
        let text = said(&message);

        let current = text.find("make ship").expect("the current one is there");
        let hedged = text.find("10.0.0.7").expect("the other one is there");
        let line = text.find(super::HEDGE).expect("the hedge is written");
        assert!(current < line, "a current memory is stated first:\n{text}");
        assert!(
            hedged > line,
            "an uncertain one is under the hedge:\n{text}"
        );
    }

    #[test]
    fn the_frame_is_there_even_when_nothing_is_current() {
        // A freshly kept memory has one witness and does not clear balthasar's assert floor, so
        // "only uncertain memories" is the *ordinary* first case, not an edge one. The frame
        // belonged to the confident section once, and this block opened "Also on record…" with
        // nothing anywhere to say it was not the conversation.
        let message = preface(&[memory("a thing somebody said once", false)], ROOMY)
            .expect("an uncertain memory is still worth saying");
        let text = said(&message);
        assert!(text.starts_with(super::PREFACE), "{text}");
        assert!(text.contains(super::HEDGE), "{text}");
    }

    #[test]
    fn a_hedge_with_nothing_under_it_is_not_written() {
        let message = preface(&[memory("only this", true)], ROOMY).expect("one memory");
        let text = said(&message);
        assert!(!text.contains(super::HEDGE), "{text}");
    }

    #[test]
    fn it_says_it_is_not_the_conversation() {
        // A model shown recalled text with no frame around it treats it as something the person
        // just said, and answers it.
        let message = preface(&[memory("a thing", true)], ROOMY).expect("one");
        assert!(said(&message).starts_with(super::PREFACE));
    }

    #[test]
    fn memory_never_costs_more_than_its_share_of_the_window() {
        // What makes this safe to do unconditionally: an injection large enough to push the
        // conversation out of the window is one that broke the turn it was informing.
        let window = 1_000;
        let many: Vec<_> = (0..500)
            .map(|i| memory(&format!("memory number {i} with some length to it"), true))
            .collect();
        let message = preface(&many, window).expect("some of them fit");
        let text = said(&message);
        assert!(
            text.len() <= window * SHARE / 100 * PER_TOKEN,
            "{} bytes of a {window}-token window",
            text.len()
        );
        assert!(text.contains("memory number 0"), "the first ones are kept");
    }

    #[test]
    fn a_window_too_small_to_share_gets_nothing() {
        // Rather than one memory that takes the whole of it.
        assert!(preface(&[memory("a thing", true)], 1).is_none());
    }

    #[test]
    fn the_confident_ones_are_written_before_the_budget_runs_out() {
        // Order matters under a budget: a hedged memory that displaced a current one would be
        // the wrong half of the answer. The window is chosen so that the heading and one line
        // fit and a second heading does not, which is where the ordering is decided.
        let window = 300;
        let mut rows: Vec<_> = (0..20)
            .map(|i| memory(&format!("uncertain {i}"), false))
            .collect();
        rows.push(memory("the current fact", true));
        let text = said(&preface(&rows, window).expect("something fits"));
        assert!(text.contains("the current fact"), "{text}");
    }
}
