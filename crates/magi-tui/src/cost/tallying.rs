//! What the cost view adds up, and what it refuses to guess.
//!
//! Split out under THE RULE; the tally is next door.

use super::*;

fn turn(at: usize, input: u64, output: u64, read: u64, write: u64) -> Turn {
    Turn {
        at,
        usage: Usage {
            input,
            output,
            cache_read: read,
            cache_write: write,
        },
    }
}

fn text(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn nothing_spent_says_so_rather_than_a_table_of_zeroes() {
    assert!(lines(&[], None).is_empty());
    assert!(empty().contains("nothing spent"));
}

#[test]
fn every_turn_is_a_row_and_the_total_is_the_sum() {
    let said = text(&lines(
        &[turn(1, 100, 20, 0, 0), turn(2, 200, 30, 50, 10)],
        None,
    ));
    assert!(said.contains("all"), "there is a total row: {said}");
    // 300 in, 50 out — the sum, not the last turn.
    assert!(said.contains("300"), "{said}");
    assert!(said.contains("50"), "{said}");
}

#[test]
fn the_four_counters_are_kept_apart() {
    // **A provider bills four things at four rates.** One "tokens" number hides the decision a
    // person can act on: a prompt that is mostly cache reads is cheap to continue and one that
    // is mostly fresh input is not.
    let said = text(&lines(&[turn(1, 10, 20, 30, 40)], None));
    for column in ["in", "out", "cache rd", "cache wr"] {
        assert!(said.contains(column), "{column} is a column: {said}");
    }
}

#[test]
fn it_says_how_much_of_the_prompt_was_cached() {
    // The one derived number worth printing: everything else is a tally, this is the ratio that
    // says whether the conversation is getting cheaper or dearer to continue.
    // 900 of 1000 prompt tokens from cache.
    let said = text(&lines(&[turn(1, 100, 5, 900, 0)], None));
    assert!(said.contains("90% of the prompt"), "{said}");
}

#[test]
fn a_session_with_no_prompt_tokens_does_not_divide_by_zero() {
    let said = text(&lines(&[turn(1, 0, 5, 0, 0)], None));
    assert!(!said.contains("% of the prompt"), "{said}");
}

#[test]
fn it_does_not_invent_a_price() {
    // magi does not know what a token costs — melchior owns the catalog, and a rate guessed here
    // would go stale the day a provider changed it, printed with the authority of one that did
    // not.
    let said = text(&lines(&[turn(1, 1_000, 100, 0, 0)], Some("anthropic/x")));
    assert!(!said.contains('$'), "no money: {said}");
    assert!(
        said.contains("providers.lua"),
        "and it says where the rate lives: {said}"
    );
    assert!(said.contains("anthropic/x"), "for this model: {said}");
}

#[test]
fn the_newest_turn_is_last() {
    // A session is read downwards; a spend table that ran the other way would be the one thing
    // on screen that did.
    let said = text(&lines(&[turn(1, 1, 0, 0, 0), turn(2, 2, 0, 0, 0)], None));
    let first = said.find("\n1 ").or_else(|| said.find("1     "));
    let second = said.find("\n2 ").or_else(|| said.find("2     "));
    assert!(first < second, "{said}");
}
