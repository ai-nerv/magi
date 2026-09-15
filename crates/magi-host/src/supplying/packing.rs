//! Packing several suppliers into one message, and saying what did not fit.
//!
//! Split out under THE RULE; the shapes these exercise are next door.

use super::*;

/// A block that is certainly citable, for a test that is about something else.
fn block(text: &str) -> Block {
    Block::new(text, "test").expect("citable").asserted(true)
}

/// One supplier, with everything it offered asserted.
fn offer(from: &str, texts: &[&str]) -> Offer {
    Offer {
        from: from.to_owned(),
        blocks: texts.iter().map(|t| block(t)).collect(),
    }
}

/// The text of what was packed.
fn said(packed: &Packed) -> String {
    packed
        .message
        .as_ref()
        .map(|m| match &m.content[0] {
            Content::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

#[test]
fn a_block_cannot_be_built_without_saying_where_it_came_from() {
    // **The rule the type exists for.** With one supplier the frame around the message said
    // where everything came from. With two, an uncited line is one the reader cannot weigh and
    // the author cannot find again — so it is refused at construction rather than rendered.
    assert_eq!(Block::new("something", ""), Err(Uncitable::Uncited));
    assert_eq!(Block::new("something", "   "), Err(Uncitable::Uncited));
    assert_eq!(Block::new("", "balthasar"), Err(Uncitable::Empty));
    assert!(Block::new("something", "balthasar").is_ok());
}

#[test]
fn every_supplier_is_named_in_the_message() {
    // What a model needs in order to weigh a claim: whether a memory layer asserted it or an
    // indexer computed it. One supplier never had to answer this.
    let packed = pack(
        &[
            offer("balthasar", &["the build command is `oslo make`"]),
            offer("index", &["`preface` is called from turn/memory.rs"]),
        ],
        200_000,
    );
    let text = said(&packed);
    assert!(text.contains("balthasar:"), "{text}");
    assert!(text.contains("index:"), "{text}");
    assert!(text.contains("the build command"), "{text}");
    assert!(text.contains("turn/memory.rs"), "{text}");
}

#[test]
fn what_does_not_fit_is_reported_rather_than_dropped_in_silence() {
    // **The failure this replaces.** The renderer before this broke out of its loop when the
    // budget ran out and told nobody — so a supplier whose answers were all slightly too long
    // looked exactly like one that found nothing, and the way to find out was to notice the
    // absence of an effect.
    let long: Vec<String> = (0..40)
        .map(|i| format!("{i} {}", "x".repeat(200)))
        .collect();
    let texts: Vec<&str> = long.iter().map(String::as_str).collect();
    let packed = pack(&[offer("balthasar", &texts)], 4_000);

    assert!(!packed.dropped.is_empty(), "something had to be dropped");
    assert!(
        packed.dropped.iter().all(|d| d.from == "balthasar"),
        "and it says whose: {:?}",
        packed.dropped
    );
}

#[test]
fn one_loud_supplier_cannot_take_the_whole_budget() {
    // The reason the budget is split rather than first-come. Before this there was one supplier
    // and the question could not arise; with two, the first one in the list would have written
    // until the budget was gone.
    let loud: Vec<String> = (0..60)
        .map(|i| format!("loud {i} {}", "y".repeat(120)))
        .collect();
    let loud_texts: Vec<&str> = loud.iter().map(String::as_str).collect();
    let packed = pack(
        &[
            offer("balthasar", &loud_texts),
            offer("index", &["the one thing the index had to say"]),
        ],
        8_000,
    );
    let text = said(&packed);
    assert!(
        text.contains("the one thing the index had to say"),
        "the quiet supplier still got in: {text}"
    );
}

#[test]
fn what_one_supplier_leaves_unspent_passes_to_the_next() {
    // Equal-then-passing, which is balthasar's own rule for sections. A fixed share per supplier
    // would waste the room a quiet one was allotted.
    let many: Vec<String> = (0..12).map(|i| format!("index fact {i}")).collect();
    let many_texts: Vec<&str> = many.iter().map(String::as_str).collect();
    let packed = pack(
        &[
            offer("balthasar", &["one short memory"]),
            offer("index", &many_texts),
        ],
        6_000,
    );
    let text = said(&packed);
    let got = many.iter().filter(|f| text.contains(f.as_str())).count();
    assert!(
        got > 6,
        "the second supplier used what the first left: {got} of 12 in\n{text}"
    );
}

#[test]
fn what_is_merely_on_record_waits_for_everything_asserted() {
    // A hedged line from the first supplier must not cost the second one its current truth.
    let hedged = Block::new("might have been the cause", "balthasar").expect("citable");
    let packed = pack(
        &[
            Offer {
                from: "balthasar".to_owned(),
                blocks: vec![hedged],
            },
            offer("index", &["this is where it is defined"]),
        ],
        200_000,
    );
    let text = said(&packed);
    let asserted_at = text.find("this is where it is defined").expect("asserted");
    let hedged_at = text.find("might have been the cause").expect("hedged");
    assert!(asserted_at < hedged_at, "hedged goes last:\n{text}");
    assert!(text.contains(HEDGE), "and under its own heading:\n{text}");
    assert!(
        text.contains("(balthasar)"),
        "still cited, even under a shared heading:\n{text}"
    );
}

#[test]
fn a_cost_nobody_could_derive_is_not_free() {
    // `None` means "nobody could work this out", which is a different claim from zero. Treating
    // the second as the first is how an uncosted block gets packed first and every time.
    let uncosted = Block::new("x".repeat(4_000), "balthasar")
        .expect("citable")
        .asserted(true);
    let packed = pack(
        &[Offer {
            from: "balthasar".to_owned(),
            blocks: vec![uncosted],
        }],
        4_000,
    );
    assert!(
        packed.message.is_none() || !packed.dropped.is_empty(),
        "it was measured rather than waved through"
    );
}

#[test]
fn nothing_offered_is_no_message_rather_than_an_empty_one() {
    // An empty block is a message that costs tokens to say nothing, every turn.
    assert!(pack(&[], 200_000).message.is_none());
    assert!(pack(&[offer("balthasar", &[])], 200_000).message.is_none());
    // And no window is no message, rather than a division by zero.
    assert!(pack(&[offer("balthasar", &["x"])], 0).message.is_none());
}

// The rest of these pin behaviour that `injecting::preface` used to hold on its own. They are
// here rather than there because the packer is what renders now, and a rule nobody re-tested
// after a move is a rule that quietly stopped applying.

#[test]
fn the_frame_is_there_even_when_nothing_is_current() {
    // **The one failure this message has to avoid.** The frame used to belong to the confident
    // section, so a recall that found only uncertain memories produced a block opening "Also on
    // record…" with nothing to say it was not the conversation — and a model shown recalled text
    // with no frame around it answers it.
    let hedged = Block::new("might be true", "balthasar").expect("citable");
    let packed = pack(
        &[Offer {
            from: "balthasar".to_owned(),
            blocks: vec![hedged],
        }],
        200_000,
    );
    let text = said(&packed);
    assert!(text.starts_with(PREFACE), "{text}");
}

#[test]
fn a_hedge_with_nothing_under_it_is_not_written() {
    // A section title with nothing beneath it tells the model there was nothing, at the price of
    // saying so.
    let packed = pack(&[offer("balthasar", &["a current fact"])], 200_000);
    let text = said(&packed);
    assert!(!text.contains(HEDGE), "{text}");
}

#[test]
fn it_says_it_is_not_the_conversation() {
    let packed = pack(&[offer("balthasar", &["a thing"])], 200_000);
    assert!(said(&packed).contains("not part of the conversation"));
}

#[test]
fn context_never_costs_more_than_its_share_of_the_window() {
    // The property that makes offering context unconditional rather than a setting somebody has
    // to find: it can never be the reason a turn overflows.
    let many: Vec<String> = (0..400).map(|i| format!("memory {i}")).collect();
    let texts: Vec<&str> = many.iter().map(String::as_str).collect();
    let window = 100_000;
    let packed = pack(&[offer("balthasar", &texts)], window);
    let text = said(&packed);
    let share = window * SHARE / 100 * PER_TOKEN;
    assert!(
        text.len() <= share,
        "{} chars against a {share} budget",
        text.len()
    );
}

#[test]
fn the_confident_ones_are_written_before_the_budget_runs_out() {
    // Order within a supplier is the supplier's, but asserted-before-hedged is the packer's: a
    // budget spent on what is merely on record is a budget not spent on what is current.
    let mut blocks = vec![Block::new("on record only", "balthasar").expect("citable")];
    blocks.extend((0..30).map(|i| {
        Block::new(format!("current fact {i}"), "balthasar")
            .expect("citable")
            .asserted(true)
    }));
    // A window tight enough that not all of it fits — which is the only situation in which the
    // order is observable at all.
    let packed = pack(
        &[Offer {
            from: "balthasar".to_owned(),
            blocks,
        }],
        1_000,
    );
    let text = said(&packed);
    assert!(text.contains("current fact 0"), "{text}");
    assert!(
        !text.contains("on record only"),
        "the hedged one did not take room from the current ones:\n{text}"
    );
    assert!(
        packed.dropped.iter().any(|d| d.text == "on record only"),
        "and it is reported rather than lost: {:?}",
        packed.dropped
    );
}
