//! What this project remembers, put in front of the model without anything having to ask for it.
//! Asserted and merely known are not the same: balthasar's `asserted` field decides which, and a
//! hedged memory stays searchable without being stated as fact. Once per prompt, not once per
//! round — a tool-using turn goes round several times and the recall is about what the person asked.

use magi_model::{Content, Message};

/// How many memories to ask for. More than fit on purpose: the budget decides what is said, and
/// asking for exactly what fits lets one long memory crowd out five short ones.
pub const MOST: u64 = 12;

/// The text of a recalled memory, trimmed.
fn text_of(row: &serde_json::Value) -> &str {
    row.get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
}

/// balthasar's answer, as an offer the packer can take beside anybody else's. A row with no text is
/// skipped rather than refused: balthasar decides what it holds.
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

/// What one injection cost: the tokens the block spends out of the window and the milliseconds the
/// turn waits. `balthasar eval` can only measure its own side; this is the half the harness pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub tokens: usize,
    pub asserted: usize,
    pub hedged: usize,
}

impl Cost {
    /// What a built preface cost, read off the message rather than the rows it was built from: the
    /// budget cuts, and a count of what was considered would report a price nobody paid.
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

/// Put what is remembered where retrieved context belongs: before the last thing said. Not at the
/// front, where a long conversation buries it, nor at the end, where it reads as the person's own.
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

    /// What a turn would actually be shown, for these rows — through the packer, as a session does.
    fn shown(rows: &[serde_json::Value], window: usize) -> Option<magi_model::Message> {
        pack(&[super::offered("balthasar", rows)], window).message
    }

    #[test]
    fn what_is_remembered_goes_before_the_last_thing_said() {
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
