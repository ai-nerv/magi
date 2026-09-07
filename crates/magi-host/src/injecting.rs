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

use magi_model::{Content, Message};

/// How many memories to ask for.
///
/// More than fit on purpose: the budget decides what is said and this decides what is
/// considered, and asking for exactly what fits would let one long memory crowd out five short
/// ones that would all have gone in.
pub const MOST: u64 = 12;

/// The text of a recalled memory, trimmed.
fn text_of(row: &serde_json::Value) -> &str {
    row.get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
}

/// balthasar's answer, as an offer the packer can take beside anybody else's.
///
/// **This is the whole of what made balthasar special, and now it is not.** Its rows went
/// straight into a renderer written around them; anything else with context to give had nowhere
/// to put it. Here they become [`crate::supplying::Block`]s cited to their supplier, and a second
/// supplier is another entry in the list rather than a second renderer.
///
/// A row with no text is skipped rather than refused: balthasar decides what it holds, and a
/// harness that failed a turn over one empty memory would be the wrong side making that call.
#[must_use]
pub fn offered(from: &str, found: &[serde_json::Value]) -> crate::supplying::Offer {
    crate::supplying::Offer {
        from: from.to_owned(),
        blocks: found
            .iter()
            .filter_map(|row| {
                let asserted = row
                    .get("asserted")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                crate::supplying::Block::new(text_of(row), from)
                    .ok()
                    .map(|block| block.asserted(asserted))
            })
            .collect(),
    }
}

/// What one injection cost, in the units a person would judge it by.
///
/// **A memory layer with no number attached is a design document.** balthasar measures whether
/// memory earns its place — `balthasar eval` answers that in success rate against a synthetic
/// project — and it can only measure its own side. What that number cannot see is the price the
/// harness pays every turn to ask: the tokens the block spends out of the window, and the
/// milliseconds the turn waits. Both are magi's, and neither was recorded anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    /// Roughly what the block will cost the window.
    pub tokens: usize,
    /// How many memories were stated as current.
    pub asserted: usize,
    /// How many were offered hedged.
    pub hedged: usize,
}

impl Cost {
    /// What a built preface cost, read off the message itself.
    ///
    /// From the message rather than from the rows it was built out of, because the budget cuts
    /// and a count of what was considered would report a price nobody paid.
    #[must_use]
    pub fn of(message: &Message) -> Self {
        let text: String = message
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let hedged_at = text.find(crate::supplying::HEDGE);
        let lines = |part: &str| part.lines().filter(|l| l.starts_with("- ")).count();
        Self {
            tokens: text.len().div_ceil(crate::supplying::PER_TOKEN),
            asserted: lines(&text[..hedged_at.unwrap_or(text.len())]),
            hedged: hedged_at.map_or(0, |at| lines(&text[at..])),
        }
    }
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
    use crate::supplying::{Offer, pack};

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

    /// What a turn would actually be shown, for these rows.
    ///
    /// Through the packer rather than a renderer of this module's own: what reaches a turn is
    /// what `Cost` has to be able to read, and a fixture built any other way would be measuring
    /// something no session produces.
    fn shown(rows: &[serde_json::Value], window: usize) -> Option<magi_model::Message> {
        pack(&[super::offered("balthasar", rows)], window).message
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
    fn a_row_with_nothing_in_it_is_skipped_rather_than_offered() {
        // balthasar decides what it holds. A harness that failed a turn over one empty memory
        // would be the wrong side making that call — and an uncitable block cannot be built, so
        // the filter has to be here rather than at the constructor's expense.
        let offer = super::offered("balthasar", &[memory("   ", true), memory("real", true)]);
        assert_eq!(offer.blocks.len(), 1);
        assert_eq!(offer.blocks[0].text(), "real");
        assert_eq!(offer.blocks[0].citation(), "balthasar");
    }

    #[test]
    fn what_balthasar_asserted_is_carried_across_as_asserted() {
        let offer = super::offered(
            "balthasar",
            &[memory("sure", true), memory("less so", false)],
        );
        assert!(offer.blocks[0].is_asserted());
        assert!(!offer.blocks[1].is_asserted());
    }

    #[test]
    fn the_cost_is_what_was_written_not_what_was_considered() {
        // Read off the message, because the budget cuts: a count of what came back from the
        // recall would report a price nobody paid. `balthasar eval` measures whether memory earns
        // its place and can only see its own side; this is the half the harness pays.
        let rows = vec![
            memory("a current fact", true),
            memory("another one", true),
            memory("something less certain", false),
        ];
        let message = shown(&rows, ROOMY).expect("three memories");

        let cost = super::Cost::of(&message);
        assert_eq!(cost.asserted, 2);
        assert_eq!(cost.hedged, 1);
        assert_eq!(
            cost.tokens,
            said(&message).len().div_ceil(crate::supplying::PER_TOKEN)
        );
    }

    #[test]
    fn what_the_budget_cut_is_not_charged_for() {
        // The reason it is read off the message. A hundred memories considered and four written
        // costs four.
        let many: Vec<_> = (0..100)
            .map(|i| memory(&format!("memory number {i}, at some length"), true))
            .collect();
        let window = 1_000;
        let message = shown(&many, window).expect("some fit");
        let cost = super::Cost::of(&message);
        assert!(cost.asserted < many.len(), "{} of 100", cost.asserted);
        assert!(
            cost.tokens <= window * crate::supplying::SHARE / 100,
            "{} tokens of a {window} window",
            cost.tokens
        );
    }

    #[test]
    fn a_supplier_that_found_nothing_puts_no_message_in_front_of_a_turn() {
        assert!(shown(&[], ROOMY).is_none());
        assert!(shown(&[memory("   ", true)], ROOMY).is_none());
        assert!(
            pack(&[] as &[Offer], ROOMY).message.is_none(),
            "and no supplier at all is the same answer"
        );
    }
}
